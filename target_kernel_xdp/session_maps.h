// target_kernel_xdp/session_maps.h
#ifndef __HOMEOSTASIS_SESSION_MAPS_H
#define __HOMEOSTASIS_SESSION_MAPS_H

#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>

/**
 * [Sovereign Buffer Cache - 64바이트 캐시라인 정렬 C ABI]
 * target_proxy_rust/src/abi.rs의 UserSessionSlot 구조체와 메모리가 1:1 완벽 호환됩니다.
 * 
 * CPU가 메모리에서 데이터를 인출할 때 64바이트 경계를 기준으로 가져오므로,
 * 이 구조체 하나는 단 하나의 캐시라인만을 차지하여 False Sharing 오염을 차단합니다.
 */
struct user_session_slot {
    /* --- Line 1: 세션 유효성 검증 가드 레일 (32바이트) --- */
    __u64 user_id;            // 유저 고유 식별자 (0: 미할당 빈 슬롯)
    __u64 expiry_tick;        // 세션 만료 타임스탬프 (틱 또는 에포크 밀리초)
    __u64 session_mask;       // 유효 세션: 0xFFFFFFFFFFFFFFFF, 차단/만료: 0x0000000000000000
    __u32 active_conn_id;     // 현재 매핑된 활성 커널 소켓 매핑 ID
    __u32 _pad1;              // 32바이트 경계 정렬 패딩

    /* --- Line 2: 라우팅 오프셋 및 동적 제어 상태 (32바이트) --- */
    __u32 profile_hash_idx;   // 하드 락킹 버퍼 내 프로필 주소 변환 고정 인덱스 (Rust 단에서 Atomic Swap 발생)
    __u32 alert_queue_head;   // 알림 분배 락프리 링버퍼 Head 포인터
    __u32 alert_queue_tail;   // 알림 분배 락프리 링버퍼 Tail 포인터
    __u32 status_flags;       // 유저 행동 상태 플래그 (0: 정상, 1: 매크로 의심, 2: 차단)
    __u8  _pad2[16];          // 총 64바이트 크기를 칼같이 맞추기 위한 최종 고정 패딩
} __attribute__((aligned(64)));

/**
 * [Sovereign Buffer Table: 고정 메모리 할당 영구 기부 맵]
 * 
 * 부팅 시점에 지정된 최대 유저 수만큼 커널 공간(Kernel Space)에 메모리를 강제 선점합니다.
 * 유저의 동시 접속 패턴이 날뛰거나 트래픽 버스트가 몰아쳐도 
 * 메모리 동적 할당(kmalloc)을 유발하지 않아 런타임 공간 복잡도가 O(1)로 동결됩니다.
 */
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);                      // Key: 고정 오프셋 인덱스 (0 ~ 9,999,999)
    __type(value, struct user_session_slot);  // Value: 64바이트 정렬 세션 데이터 구조체
    __uint(max_entries, 10000000);           // 동시 접속 가용 용량 고정 (예: 최대 1,000만 슬롯)
} session_array_map SEC(".maps");

#endif /* __HOMEOSTASIS_SESSION_MAPS_H */
