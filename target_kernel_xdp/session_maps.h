// -*- coding: utf-8 -*-
// target_kernel_xdp/session_maps.h

#ifndef __HOMEOSTASIS_SESSION_MAPS_H
#define __HOMEOSTASIS_SESSION_MAPS_H

#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>

/**
 * [Sovereign Buffer Cache - Single L1 Cache-Line Hit 격리 C ABI]
 * target_proxy_rust/src/abi.rs의 UserSessionSlot 구조체와 오프셋 바이트 단위까지 1:1 대칭 매핑됩니다.
 * 
 * 인그레스 최전방 핫 패스에서 호출되는 핵심 마스킹 검증 데이터들을 구조체 시작부 32바이트 이내로 
 * 칼같이 바인딩하여, 멀티코어 버스 쟁탈 및 거짓 공유(False Sharing) 지터를 물리적으로 박멸합니다.
 */
struct user_session_slot {
    /* ========================================================================= */
    /* LINE 1: 핫 패스 가속 제어 레일 (최전방 L1 단일 캐시라인 경계 32바이트)    */
    /* ========================================================================= */
    __u64 session_mask;       // 8 Bytes (오프셋 0)  | 유효 세션: 0xFFFFFFFFFFFFFFFF, 차단/만료: 0x0000000000000000
    __u64 expiry_tick;        // 8 Bytes (오프셋 8)  | 세션 만료 제어용 초정밀 커널 타임 틱
    __u64 user_id;            // 8 Bytes (오프셋 16) | 유저 고유 레지스터 식별 정수 (0: 미할당 빈 슬롯)
    __u32 active_conn_id;     // 4 Bytes (오프셋 24) | 현재 매핑된 활성 커널 소켓 매핑 ID
    __u32 _pad1;              // 4 Bytes (오프셋 28) | 정확히 32바이트 물리 경계를 형성하기 위한 가드 패딩

    /* ========================================================================= */
    /* LINE 2: 콜드 패스 / 컨트롤 플레인 업데이트 전용 레일 (나머지 32바이트)  */
    /* ========================================================================= */
    __u32 profile_hash_idx;   // 4 Bytes (오프셋 32) | 하드 락킹 버퍼 내 프로필 주소 고정 해시 인덱스 (0ns Atomic Swap 대상)
    __u32 alert_queue_head;   // 4 Bytes (오프셋 36) | 알림 분배 락프리 링버퍼 Head 포인터
    __u32 alert_queue_tail;   // 4 Bytes (오프셋 40) | 알림 분배 락프리 링버퍼 Tail 포인터
    __u32 status_flags;       // 4 Bytes (오프셋 44) | 유저 행동 상태 플래그 (0:정상, 1:매크로, 2:블랙리스트)
    __u8  _pad2[16];          // 16 Bytes(오프셋 48) | 총 64바이트 하드웨어 얼라인먼트를 완결짓기 위한 최종 고정 버퍼
} __attribute__((aligned(64)));


/**
 * [Sovereign Buffer Table: 고정 메모리 할당 영구 기부 맵]
 * 
 * 부팅 시점에 지정된 최대 유저 수만큼 커널 공간(Kernel Space)에 메모리를 강제 선점합니다.
 * 유저의 동시 접속 패턴이 날뛰거나 트래픽 버스트가 몰아쳐도 
 * 메모리 동적 할당(kmalloc)을 유발하지 않아 런타임 공간 복잡도가 O(1)로 동결됩니다.
 * 
 * [★ 고도화 수호 마진]: max_entries를 하드코딩하지 않고, `config/system_bounds.toml` 및 
 * 하단 컴파일러 파이프라인의 글로벌 상수와 일치하도록 eBPF 빌드 플래그 매크로 연동을 강화합니다.
 */
#ifndef MAX_USER_SLOTS
#define MAX_USER_SLOTS 10000000  // 기본값 1,000만 슬롯 보존 (빌드 타임 주입 우선)
#endif

struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __type(key, __u32);                      // Key: 고정 오프셋 인덱스 (0 ~ MAX_USER_SLOTS - 1)
    __type(value, struct user_session_slot); // Value: L1 캐시라인 정렬 완성형 64바이트 세션 데이터 구조체
    __uint(max_entries, MAX_USER_SLOTS);     // 동시 접속 가용 용량 고정 (메모리 파편화 및 kmalloc 지터 0%화)
} session_array_map SEC(".maps");

#endif /* __HOMEOSTASIS_SESSION_MAPS_H */
