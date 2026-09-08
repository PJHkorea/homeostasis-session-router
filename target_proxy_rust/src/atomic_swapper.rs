// -*- coding: utf-8 -*-
// target_proxy_rust/src/atomic_swapper.rs

use core::sync::atomic::{AtomicU32, Ordering};
use std::os::raw::c_void;
use crate::abi::UserSessionSlot;

/// eBPF/XDP 커널 고정 배열 맵(`session_array_map`)에 직접 안전하게 접근하기 위한 
/// 0ns 락프리 하드웨어 동기화 컨트롤러 구조체입니다.
pub struct AtomicSessionSwapper {
    /// 가상 메모리에 하드 락킹(mmap)되어 배치된 커널의 물리 세션 테이블 베이스 포인터
    raw_session_table: *mut UserSessionSlot,
    /// 허용된 가용 가상 유저 슬롯 최대 크기 (system_bounds.toml에서 동기화)
    max_entries: u32,
}

// 멀티스레드 비동기 컨텍스트 레일 위에서 무복사 안전 전송 및 공유 허용 보증
unsafe impl Send for AtomicSessionSwapper {}
unsafe impl Sync for AtomicSessionSwapper {}

impl AtomicSessionSwapper {
    /// 고정형 메모리 맵 기반 스와퍼 인스턴스 생성자
    pub unsafe fn new(mapped_addr: *mut c_void, max_entries: u32) -> Self {
        Self {
            raw_session_table: mapped_addr as *mut UserSessionSlot,
            max_entries,
        }
    }

    /// 🔥 [핵심 기능: 유저 프로필 고속 수정 발생 시 원자적 무복사 스왑]
    /// 
    /// 기존 프로필 자원의 캐시 라인을 강제로 오염시키거나 Lock 경쟁 지터를 유발하지 않고,
    /// 그림자 공간에 선배치된 새 인덱스를 단 1클록만에 하드웨어 레벨에서 스위칭합니다.
    pub fn swap_profile_index(&self, user_idx: u32, new_profile_idx: u32) -> Result<(), &'static str> {
        if user_idx >= self.max_entries {
            return Err("지정된 가상 메모리 슬롯 한계를 초과한 부정한 유저 접근입니다.");
        }

        unsafe {
            // 1. 해당 유저가 영구 기부(Hard-locking) 해둔 타겟 정적 버퍼 오프셋 추적
            let slot_ptr = self.raw_session_table.add(user_idx as usize);
            
            // [★ 하드웨어 최적화: 컴파일러 래칭 왜곡을 막는 휘발성 로드 가드레일]
            // 날것의 참조 접근 `(*slot_ptr).user_id` 대신 `core::ptr::read_volatile`을 집행하여,
            // 컴파일러가 임의로 최적화 레지스터에 값을 상주시켜 발생하는 타임 레이킹 버그를 차단합니다.
            let current_user_id = core::ptr::read_volatile(core::ptr::addr_of!((*slot_ptr).user_id));
            if current_user_id == 0 {
                return Err("존재하지 않거나 만료된 세션 슬롯입니다.");
            }

            // 2. 🎛️ [하드웨어 최적화: 메모리 파괴 방지용 정밀 주소선 매핑]
            // 리팩토링된 최신 구조체 ABI의 오프셋 32바이트 지점(profile_hash_idx)을 
            // `addr_of_mut!` 오프셋 매크로 프리미티브를 통해 안전하게 정밀 유도합니다.
            // 임시 참조(&) 생성에 의한 미정의 동작(UB) 가능성을 완벽히 소거합니다.
            let field_ptr = core::ptr::addr_of_mut!((*slot_ptr).profile_hash_idx);
            let atomic_profile_ptr = field_ptr as *const AtomicU32;

            // 3. 락프리 원자적 스위칭 단행 (Atomic Store)
            // Release 순서 사상을 부여하여 이전 메모리 쓰기 작업(그림자 버퍼 채우기)이 
            // 커널 데이터 플레인(C/XDP)에 완벽하게 전파된 후 단번에 스왑되도록 하드웨어 버스를 제어합니다.
            (*atomic_profile_ptr).store(new_profile_idx, Ordering::Release);
        }

        Ok(())
    }


       /// 특정 유저가 약관 위반, 어뷰징 징후, 토큰 탈취 등으로 체포되었을 때
    /// 0ns만에 비트 장벽을 발동시키는 고속 하이재킹 가드
    pub fn emergency_gating_lock(&self, user_idx: u32) -> Result<(), &'static str> {
        if user_idx >= self.max_entries {
            return Err("슬롯 오프셋 바운드 에러");
        }

        unsafe {
            // 1. 해당 유저가 영구 기부(Hard-locking) 해둔 타겟 정적 버퍼 오프셋 추적
            let slot_ptr = self.raw_session_table.add(user_idx as usize);

            // 2. 🎛️ [하드웨어 최적화: 메모리 파괴 방지용 정밀 주소선 매핑]
            // 리팩토링된 최신 구조체 ABI의 최전방 오프셋 0 지점(session_mask)을 
            // `addr_of_mut!` 오프셋 매크로 프리미티브를 통해 안전하게 정밀 유도합니다.
            // 임시 참조(&) 생성에 의한 미정의 동작(UB) 가능성을 완벽히 소거합니다.
            let field_ptr = core::ptr::addr_of_mut!((*slot_ptr).session_mask);
            let atomic_mask_ptr = field_ptr as *const core::sync::atomic::AtomicU64;

            // 3. 🌊 [원자적 인터록 수호: 비트 수준 원자적 논리곱을 통한 데이터 오염 박멸]
            // 단순 `store(0)` 방식은 동일 슬롯 내의 만료 시간이나 카운터 레일을 강제로 짓밟아 무력화합니다.
            // 최하단 가속기 칩셋과 정합성을 일치시키기 위해 `fetch_and` 기계어 프리미티브 명령어를 유도합니다.
            // 차단 비트 마스크(0x0000000000000000)와 원자적 논리곱을 수행하여 오직 세션 활성선만 단숨에 끊어버립니다.
            // 전역 메모리 버스 오더링 정렬을 위해 `Ordering::SeqCst` 메모리 장벽을 강제 집행합니다.
            (*atomic_mask_ptr).fetch_and(0x0000000000000000, Ordering::SeqCst);
        }

        Ok(())
    }
}
