// target_kernel_xdp/bitwise_session_mux.c
#include <linux/bpf.h>
#include <linux/in.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <bpf/bpf_helpers.h>

/* Rust의 UserSessionSlot 구조체와 1:1 매핑되는 64바이트 Aligned C 구조체 */
struct user_session_slot {
    // 1번 캐시라인 패스 (32바이트)
    __u64 user_id;
    __u64 expiry_tick;
    __u64 session_mask; // 유효: 0xFFFFFFFFFFFFFFFF, 만료/차단: 0x0000000000000000
    __u32 active_conn_id;
    __u32 _pad1;

    // 2번 캐시라인 패스 (32바이트)
    __u32 profile_hash_idx;
    __u32 alert_queue_head;
    __u32 alert_queue_tail;
    __u32 status_flags;
    __u8  _pad2[16];
} __attribute__((aligned(64)));

/* 초고속 O(1) 룩업을 위한 eBPF 고정 정적 배열 맵 정의 */
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);                 // Key: 유저의 내부 가상 인덱스 ID
    __type(value, struct user_session_slot); // Value: Hard-locked 세션 버퍼
    __uint(max_entries, 10000000);      // 예시: 최대 1,000만 명의 고정 슬롯 기부 선점
} session_array_map SEC(".maps");

SEC("xdp")
int bitwise_session_mux_ingress(struct xdp_md *ctx) {
    void *data_end = (void *)(long)ctx->data_end;
    void *data = (void *)(long)ctx->data;

    // 1. 기본적인 네트워크 패킷 파싱 및 안전성 검사 (eBPF 검증기 필수 통과 구간)
    struct ethhdr *eth = data;
    if ((void *)(eth + 1) > data_end) return XDP_PASS;

    if (eth->h_proto != __constant_htons(ETH_P_IP)) return XDP_PASS;

    struct iphdr *iph = data + sizeof(struct ethhdr);
    if ((void *)(iph + 1) > data_end) return XDP_PASS;

    // TCP 패킷만 추출
    if (iph->protocol != IPPROTO_TCP) return XDP_PASS;

    struct tcphdr *th = data + sizeof(struct ethhdr) + sizeof(struct iphdr);
    if ((void *)(th + 1) > data_end) return XDP_PASS;

    // 2. 가상의 페이로드 추출 메커니즘 
    // (실제 구현 시에는 패킷 바디의 특정 오프셋에서 유저 고유 가상 인덱스를 추출)
    // 여기서는 예시를 위해 TCP 포트 번호 등을 조합하여 임의의 가상 user_idx를 산출했다고 가정합니다.
    __u32 user_idx = __constant_ntohs(th->source) % 10000000; 

    // 3. O(1) 초고속 세션 버퍼 참조 (메모리 할당 변동 없음)
    struct user_session_slot *slot = bpf_map_lookup_elem(&session_array_map, &user_idx);
    if (!slot) {
        return XDP_DROP; // 할당되지 않은 부정한 슬롯 주소 접근 시 즉시 무복사 소산
    }

    // 4. 🔥 [재미있는 포인트] 조건문(if) 없는 분기 거세 게이팅 (Branchless Gating)
    // 레거시 방식: if (slot->session_mask == 0) { return XDP_DROP; }
    // 아래 비트 MUX 식은 CPU의 JMP 명령어를 사용하지 않고, 오직 레지스터 단의 AND/OR/NOT 연산으로만 귀결됩니다.
    
    __u64 mask = slot->session_mask; // 0xFFFFFFFFFFFFFFFF 또는 0x0000000000000000
    
    // 연산의 결과는 오직 mask의 상태에 따라 하드웨어 비트 스위칭으로 결정됩니다.
    // mask가 유효(0xFF...)하면 XDP_PASS(2)가 남고, 만료(0x00...)하면 XDP_DROP(1)이 최종 선택됩니다.
    int action = (XDP_PASS & mask) | (XDP_DROP & ~mask);

    return action; 
}

char _license[] SEC("license") = "GPL";
