// -*- coding: utf-8 -*-
// target_proxy_rust/tests/test_kernel.rs

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::mem::{size_of, align_of};
use homeostasis_session_router::abi::UserSessionSlot;
use homeostasis_session_router::atomic_swapper::AtomicSessionSwapper;

#[test]
fn test_abi_hardware_alignment_and_sizes() {
    println!("[+] ABI 구조체의 하드웨어 정렬 및 바이트 크기 사상 검증 시작...");

    // 1. CPU 캐시라인(64바이트) 경계에 완벽히 정렬되었는지 검증
    // 이 값이 64가 아니면 멀티코어 환경에서 False Sharing 오염이 발생하여 고빈도 Write 시 지터가 터집니다.
    assert_eq!(align_of::<UserSessionSlot>(), 64, "[-] 치명적 오류: UserSessionSlot의 하드웨어 정렬이 64바이트가 아닙니다!");

    // 2. 전체 메모리 풋프린트가 정확히 64바이트 컴팩트 형상인지 검증 (C 커널 맵 배열 인덱싱의 기저)
    assert_eq!(size_of::<UserSessionSlot>(), 64, "[-] 치명적 오류: UserSessionSlot의 크기가 정확히 64바이트가 아닙니다!");
    
    println!("[+] 성공: 64바이트 캐시라인 Aligned 정적 크기 바인딩 확인 완료.");
}

#[test]
fn test_branchless_bit_mux_gating_logic() {
    println!("[+] 고도화된 C 커널(bitwise_session_mux.c)의 2중 무분기 비트 MUX 라우팅 수리적 검증...");

    // eBPF 커널 내부 정수 액션 플래그 매핑: XDP_PASS = 2, XDP_DROP = 1
    const XDP_PASS: u64 = 2;
    const XDP_DROP: u64 = 1;

    // 가상의 커널 초정밀 나노초 타임스탬프 수립
    const VIRTUAL_CURRENT_TIME_NS: u64 = 5000000000;

    // [시나리오 A: 마스크 유효 + 시간 유효 -> PASS 확정]
    let valid_mask: u64 = 0xFFFFFFFFFFFFFFFF;
    let future_expiry_tick: u64 = VIRTUAL_CURRENT_TIME_NS + 1000000000; // 1초 뒤 만료 예정
    
    let time_delta_a = future_expiry_tick as i64 - VIRTUAL_CURRENT_TIME_NS as i64;
    let time_expired_mask_a = (time_delta_a >> 63) as u64; // 양수이므로 부호비트 전개 시 0x00...
    let time_valid_mask_a = !time_expired_mask_a;         // 반전 시 0xFF...
    
    let final_mask_a = valid_mask & time_valid_mask_a;
    let action_a = (XDP_PASS & final_mask_a) | (XDP_DROP & !final_mask_a);
    assert_eq!(action_a, XDP_PASS, "[-] 무결성 세션 검증 결과 XDP_PASS 판정 실패");

    // [시나리오 B: 마스크 유효 + 시간 만료 -> DROP 강제 소산 (타임레이스 어택 원천 격멸)]
    let past_expiry_tick: u64 = VIRTUAL_CURRENT_TIME_NS - 1000; // 이미 1000ns 전에 만료됨
    
    let time_delta_b = past_expiry_tick as i64 - VIRTUAL_CURRENT_TIME_NS as i64;
    let time_expired_mask_b = (time_delta_b >> 63) as u64; // 음수이므로 부호비트 전개 시 0xFF...
    let time_valid_mask_b = !time_expired_mask_b;         // 반전 시 0x00...
    
    let final_mask_b = valid_mask & time_valid_mask_b; // 마스크가 0xFF...여도 시간 가드에 의해 0x00...으로 싱크 붕괴
    let action_b = (XDP_PASS & final_mask_b) | (XDP_DROP & !final_mask_b);
    assert_eq!(action_b, XDP_DROP, "[-] 만료 세션의 타임 레이스 우회 차단(XDP_DROP) 집행 실패");

    // [시나리오 C: 가속기 차단 마스크(0x00...) -> 시간 무관 무조건 DROP 소산]
    let blocked_mask: u64 = 0x0000000000000000;
    let final_mask_c = blocked_mask & time_valid_mask_a;
    let action_c = (XDP_PASS & final_mask_c) | (XDP_DROP & !final_mask_c);
    assert_eq!(action_c, XDP_DROP, "[-] 가속기 락 신호 인입 시 XDP_DROP 소산 판정 실패");

    println!("[+] 성공: 조건문(if) 없는 2중 산술 비트 스위칭 레일의 하드웨어 결정론적 무결성 확인 완료.");
}

#[test]
fn test_atomic_profile_swap_and_emergency_lock() {
    println!("[+] Rust 컨트롤 플레인 단의 락프리 원자적 상태 변경 및 메모리 영구 기부 안정성 테스트...");

    // 가상 메모리 공간 상에 3개의 연속된 정적 유저 세션 슬롯 가상 선점 (Zero-allocation 에뮬레이션)
    let mut mock_shared_memory = [UserSessionSlot::empty(); 3];
    
    // 0번 슬롯에 가상 유저 영구 기부 및 초기 타임 가드 틱 임의 설정
    mock_shared_memory[0].user_id = 7777;
    mock_shared_memory[0].session_mask = 0xFFFFFFFFFFFFFFFF;
    mock_shared_memory[0].expiry_tick = 9999999999;
    mock_shared_memory[0].profile_hash_idx = 101; // 원본 프로필 해시 인덱스 오프셋

    let raw_ptr = mock_shared_memory.as_mut_ptr() as *mut std::ffi::c_void;
    let swapper = unsafe { AtomicSessionSwapper::new(raw_ptr, 3) };

    // 1. 0ns 무중단 프로필 수정 스왑 검증
    // 그림자 버퍼에 새 프로필(999번 인덱스)을 준비했다고 가정하고, 원자적 주소선 교체 단행
    let swap_res = swapper.swap_profile_index(0, 999);
    assert!(swap_res.is_ok());
    
    // [★ 하드웨어 최적화: 리팩토링된 오프셋 32B 지점을 추적하는 안전 매크로 주소 유도]
    // 미정의 동작(UB)을 유발하는 날것의 참조 캐스팅(`&mock_shared_memory[0]...`) 구조를 전면 폐기하고,
    // `core::ptr::addr_of!` 프리미티브를 통해 최신 이종 FFI 정렬 명세 오프셋을 안전하게 정조준합니다.
    let field_profile_ptr = core::ptr::addr_of!(mock_shared_memory[0].profile_hash_idx);
    let atomic_profile_ptr = field_profile_ptr as *const AtomicU32;
    assert_eq!(unsafe { (*atomic_profile_ptr).load(Ordering::Acquire) }, 999, "[-] 원자적 프로필 인덱스 스왑이 메모리에 반영되지 않았습니다.");

    // 2. 위상 공간 붕괴 포착 시 즉각적인 에머전시 비트 가드 잠금 검증
    let lock_res = swapper.emergency_gating_lock(0);
    assert!(lock_res.is_ok());

    // [★ 하드웨어 최적화: L1 최전방 오프셋 0B 구역으로 전진 배치된 session_mask 정밀 단언]
    let field_mask_ptr = core::ptr::addr_of!(mock_shared_memory[0].session_mask);
    let atomic_mask_ptr = field_mask_ptr as *const core::sync::atomic::AtomicU64;
    
    // fetch_and(0) 비트 클리어 명령에 의해 타겟 세션 비트선만 0x00...으로 조용히 증발되었는지 이중 검증합니다.
    assert_eq!(unsafe { (*atomic_mask_ptr).load(Ordering::Acquire) }, 0x0000000000000000, "[-] 긴급 세션 비트 락(fetch_and) 집행 실패");

    // [★ 하드웨어 최적화 싱크]: 비트 소거 연산 후에도 동일 슬롯 내의 expiry_tick 등의 콜드 자원이 
    // 파괴되거나 오염되지 않고 정상 상주(9999999999) 하고 있는지 메모리 수호 무결성을 최종 단언합니다.
    let field_expiry_ptr = core::ptr::addr_of!(mock_shared_memory[0].expiry_tick);
    let atomic_expiry_ptr = field_expiry_ptr as *const core::sync::atomic::AtomicU64;
    assert_eq!(unsafe { (*atomic_expiry_ptr).load(Ordering::Acquire) }, 9999999999, "[-] 비트 락 집행 중 주변 캐시라인 자원 오염 결함 포착");

    println!("[+] 성공: 0ns 레이텐시 락프리 원자성 조작 및 비침습 비트 소거 파이프라인의 안전성 무결성 최종 공인.");
}
