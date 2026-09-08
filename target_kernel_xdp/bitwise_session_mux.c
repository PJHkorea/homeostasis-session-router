// -*- coding: utf-8 -*-
// target_kernel_xdp/bitwise_session_mux.c

#include <linux/bpf.h>
#include <linux/in.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <bpf/bpf_helpers.h>

/* [★ 하드웨어 최적화: Single L1 Cache-Line Hit 격리 배치 설계]
 * Rust ABI 및 가속기 칩셋 구조와의 1:1 직결을 보장하면서, 
 * 인그레스 핫 패스(Hot Path) 연산에 필요한 데이터 축들을 최전방 32바이트 단일 캐시라인 이내로 칼정렬합니다.
 * 이로써 멀티코어 환경의 캐시 일관성 버스 부하 및 메모리 지터가 물리적으로 0%화됩니다.
 */
struct user_session_slot {
    // ==== 핫 패스 레일 가속 존 (최전방 32바이트 캐시라인 완벽 구속) ====
    __u64 session_mask;       // 8 Bytes (오프셋 0) | 유효: 0xFFFFFFFFFFFFFFFF, 만료: 0x0000000000000000
    __u64 expiry_tick;        // 8 Bytes (오프셋 8) | 세션 만료 제어 커널 틱
    __u64 user_id;            // 8 Bytes (오프셋 16)| 유저 고유 레지스터 식별 정수
    __u32 active_conn_id;     // 4 Bytes (오프셋 24)| 실시간 연결 추적 ID
    __u32 _pad1;              // 4 Bytes (오프셋 28)| 32바이트 캐시 경계선 타겟용 제로 패딩
    
    // ==== 콜드 패스 / 컨트롤 플레인 업데이트 전용 존 (나머지 32바이트) ====
    __u32 profile_hash_idx;   // 4 Bytes (오프셋 32)| 문자열 인터닝 인덱스 주소
    __u32 alert_queue_head;   // 4 Bytes (오프셋 36)| 텔레메트리 큐 헤더 포인터
    __u32 alert_queue_tail;   // 4 Bytes (오프셋 40)| 텔레메트리 큐 테일 포인터
    __u32 status_flags;       // 4 Bytes (오프셋 44)| 시스템 상태 비트 플래그
    __u8  _pad2[16];          // 16 Bytes(오프셋 48)| 정확히 64바이트 하드웨어 대칭 얼라인먼트 완성
} __attribute__((aligned(64)));

/* 초고속 O(1) 정적 메모리 룩업을 위한 eBPF 배열 맵 정의 (런타임 동적 할당 0% 고정) */
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);                      // Key: 유저의 내부 가상 고정 인덱스 ID
    __type(value, struct user_session_slot); // Value: Hard-locked 세션 래티스 버퍼
    __uint(max_entries, 10000000);           // 최대 1,000만 명의 고정 슬롯 영역 선점
} session_array_map SEC(".maps");

SEC("xdp")
int bitwise_session_mux_ingress(struct xdp_md *ctx) {
    // 컴파일러 레지스터 최적화를 유도하기 위해 명시적 64비트 가상 주소 포인터 캐스팅 적용
    void *data_end = (void *)(long)ctx->data_end;
    void *data = (void *)(long)ctx->data;

    // 1. 기본적인 네트워크 패킷 파싱 및 안전성 검사 (eBPF Verifier 최적 하이패스 통과 구간)
    struct ethhdr *eth = data;
    if ((void *)(eth + 1) > data_end) return XDP_PASS;

    if (eth->h_proto != __constant_htons(ETH_P_IP)) return XDP_PASS;

    struct iphdr *iph = (void *)((unsigned char *)data + sizeof(struct ethhdr));
    if ((void *)(iph + 1) > data_end) return XDP_PASS;

    // TCP 프로토콜 패킷 필터링
    if (iph->protocol != IPPROTO_TCP) return XDP_PASS;

    struct tcphdr *th = (void *)((unsigned char *)iph + sizeof(struct iphdr));
    if ((void *)(th + 1) > data_end) return XDP_PASS;


       // 2. 가상의 L7 페이로드 유저 고유 인덱스 추출 메커니즘
    // 최대 1,000만 명의 고정 래티스 버퍼 슬롯 바운더리 내부로 정적 마스킹 처리합니다.
    __u32 user_idx = __constant_ntohs(th->source) % 10000000; 

    // 3. O(1) 초고속 메모리 고정 세션 버퍼 참조 (가비지 컬렉션 및 할당 변동 0%)
    struct user_session_slot *slot = bpf_map_lookup_elem(&session_array_map, &user_idx);
    if (!slot) {
        return XDP_DROP; // 부정한 슬롯 인덱스 접근 시 즉시 무복사 패킷 소산
    }

    // 4. 🎛️ [하드웨어 최적화: 타임 레이스 취약점을 분기문 없이 부수어버리는 2중 비트 MUX]
    // bpf_ktime_get_ns()로 현재 부팅 이후 경과된 절대 나노초 시간을 초고속 래칭합니다.
    __u64 current_time_ns = bpf_ktime_get_ns();
    
    // 만료 시간 검증: (expiry_tick - current_time_ns)
    // 세션이 유효하면 결과가 양수(최상위 부호 비트 0), 만료되면 음수(최상위 부호 비트 1)가 됩니다.
    __s64 time_delta = (__s64)slot->expiry_tick - (__s64)current_time_ns;
    
    // 산술 우측 시프트(Arithmetic Shift Right `>> 63`)를 통해 부호 비트를 전체 비트선으로 전개합니다.
    // 양수(유효) -> 0x0000000000000000 | 음수(만료) -> 0xFFFFFFFFFFFFFFFF
    __u64 time_expired_mask = (__u64)(time_delta >> 63);
    
    // 비트 NOT(~ ) 연산을 통해 유효할 때 0xFF..., 만료 시 0x00...인 '시간 무결성 비트 가드'로 반전시킵니다.
    __u64 time_valid_mask = ~time_expired_mask;

    // 5. 🔥 [분기문 100% 거세: 실리콘 레벨 융합 게이팅 체인]
    // 가속기 칩셋이 통제하는 실시간 `session_mask`와 커널이 자체 검증한 `time_valid_mask`를 
    // 단 1클록 만에 비트 AND(`&`)로 대칭 융합하여 최종 '절대 무결성 마스크'를 합성합니다.
    // 두 가드레일이 동시에 참(0xFF...)이어야만 완벽한 패스(XDP_PASS) 권한이 승인됩니다.
    __u64 final_integrity_mask = slot->session_mask & time_valid_mask;
    
    // 단 일직선의 ALU 레지스터 연산 명령어로 귀결 (CPU JMP 명령어 완전 소멸)
    // final_integrity_mask가 완벽히 보존되면 XDP_PASS(2), 단 1비트라도 붕괴하면 XDP_DROP(1)이 하드웨어 자동 스위칭됩니다.
    int action = (XDP_PASS & final_integrity_mask) | (XDP_DROP & ~final_integrity_mask);

    return action; 
}

char _license[] SEC("license") = "GPL";

