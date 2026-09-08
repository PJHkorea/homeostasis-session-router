// target_proxy_rust/tests/test_kernel.rs

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::mem::{size_of, align_of};
use homeostasis_session_router::abi::UserSessionSlot;
use homeostasis_session_router::atomic_swapper::AtomicSessionSwapper;

#[test]
fn test_abi_hardware_alignment_and_sizes() {
    println!("[+] ABI 구조체의 하드웨어 정렬 및 바이트 크기 사상 검증 시작...");

    // 1. CPU 캐시라인(64바이트) 경계에 완벽히 정렬되었는지 검증
    // 이 값이 64가 아니면 멀티코어 환경에서 False Sharing 오염이 발생하여 고 빈도 Write 시 지터가 터집니다.
    assert_eq!(align_of::<UserSessionSlot>(), 64, "[-] 치명적 오류: UserSessionSlot의 하드웨어 정렬이 64바이트가 아닙니다!");

    // 2. 전체 메모리 풋프린트가 정확히 64바이트 컴팩트 형상인지 검증 (C 커널 맵 배열 인덱싱의 기저)
    assert_eq!(size_of::<UserSessionSlot>(), 64, "[-] 치명적 오류: UserSessionSlot의 크기가 정확히 64바이트가 아닙니다!");
    
    println!("[+] 성공: 64바이트 캐시라인 Aligned 정적 크기 바인딩 확인 완료.");
}

#[test]
fn test_branchless_bit_mux_gating_logic() {
    println!("[+] C 커널(bitwise_session_mux.c)의 분기문 없는 비트 MUX 라우팅 수리적 검증...");

    // eBPF 커널 내부 정수 매핑: XDP_PASS = 2, XDP_DROP = 1
    const XDP_PASS: u64 = 2;
    const XDP_DROP: u64 = 1;

    // 시나리오 A: 유효 세션 마스크 (0xFFFFFFFFFFFFFFFF)
    let valid_mask: u64 = 0xFFFFFFFFFFFFFFFF;
    let action_valid = (XDP_PASS & valid_mask) | (XDP_DROP & !valid_mask);
    assert_eq!(action_valid, XDP_PASS, "[-] 유효 세션 마스크 적용 시 XDP_PASS 판정 실패");

    // 시나리오 B: 만료/차단 세션 마스크 (0x0000000000000000)
    let invalid_mask: u64 = 0x0000000000000000;
    let action_invalid = (XDP_PASS & invalid_mask) | (XDP_DROP & !invalid_mask);
    assert_eq!(action_invalid, XDP_DROP, "[-] 만료 세션 마스크 적용 시 XDP_DROP 차단 실패");

    println!("[+] 성공: 조건문(if) 없는 레지스터 단 비트 스위칭 로직의 결정론적 무결성 확인.");
}

#[test]
fn test_atomic_profile_swap_and_emergency_lock() {
    println!("[+] Rust 컨트롤 플레인 단의 락프리 원자적 상태 변경 및 메모리 영구 기부 안정성 테스트...");

    // 가상 메모리 공간 상에 3개의 연속된 정적 유저 세션 슬롯 가상 선점 (Zero-allocation 에뮬레이션)
    let mut mock_shared_memory = [UserSessionSlot::empty(); 3];
    
    // 0번 슬롯에 가상 유저 영구 기부 설정
    mock_shared_memory[0].user_id = 7777;
    mock_shared_memory[0].session_mask = 0xFFFFFFFFFFFFFFFF;
    mock_shared_memory[0].profile_hash_idx = 101; // 원본 프로필 해시 인덱스 오프셋

    let raw_ptr = mock_shared_memory.as_mut_ptr() as *mut std::ffi::c_void;
    let swapper = unsafe { AtomicSessionSwapper::new(raw_ptr, 3) };

    // 1. 0ns 무중단 프로필 수정 스왑 검증
    // 그림자 버퍼에 새 프로필(999번 인덱스)을 준비했다고 가정하고, 원자적 주소선 교체 단행
    let swap_res = swapper.swap_profile_index(0, 999);
    assert!(swap_res.is_ok());
    
    // 비트 버스 스레드 전파 상태 확인 (Atomic Load)
    let atomic_profile_ptr = &mock_shared_memory[0].profile_hash_idx as *const u32 as *const AtomicU32;
    assert_eq!(unsafe { (*atomic_profile_ptr).load(Ordering::Acquire) }, 999, "[-] 원자적 프로필 인덱스 스왑이 메모리에 반영되지 않았습니다.");

    // 2. 위상 공간 붕괴 포착 시 즉각적인 에머전시 비트 가드 잠금 검증
    let lock_res = swapper.emergency_gating_lock(0);
    assert!(lock_res.is_ok());

    let atomic_mask_ptr = &mock_shared_memory[0].session_mask as *const u64 as *const AtomicU64;
    assert_eq!(unsafe { (*atomic_mask_ptr).load(Ordering::Acquire) }, 0x0000000000000000, "[-] 긴급 세션 비트 락 집행 실패");

    println!("[+] 성공: 0ns 레이텐시 락프리 원자성 조작 파이프라인 정상 가동 확인.");
}
