// target_proxy_rust/src/atomic_swapper.rs

use core::sync::atomic::{AtomicU32, Ordering};
use std::os::raw::c_void;
use crate::abi::UserSessionSlot;

/// eBPF/XDP 커널 고정 배열 맵(`session_array_map`)에 직접 안전하게 접근하기 위한 
/// 0ns 컨트롤러 구조체입니다.
pub struct AtomicSessionSwapper {
    /// 메모리에 하드 락킹(mmap 등)되어 배치된 커널의 물리 세션 테이블 포인터
    raw_session_table: *mut UserSessionSlot,
    /// 허용된 가용 가상 유저 슬롯 최대 크기 ( system_bounds.toml에서 동기화 )
    max_entries: u32,
}

// 스레드 간 안전한 메모리 참조 이동 허용
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

    /// 🔥 [핵심 기능] 유저 프로필 고속 수정 발생 시 원자적 무복사 스왑 (Atomic Profile Swap)
    /// 
    /// 기존 프로필 자원의 캐시 라인을 강제로 오염시키거나 Lock을 걸지 않고,
    /// 그림자 공간(Shadow Slot)에 먼저 준비된 새 인덱스를 1클록만에 하드웨어 레벨에서 스위칭합니다.
    pub fn swap_profile_index(&self, user_idx: u32, new_profile_idx: u32) -> Result<(), &'static str> {
        if user_idx >= self.max_entries {
            return Err("지정된 가상 메모리 슬롯 한계를 초과한 부정한 유저 접근입니다.");
        }

        unsafe {
            // 1. 해당 유저가 영구 기부(Hard-locking) 해둔 타겟 정적 버퍼 오프셋 추적
            let slot_ptr = self.raw_session_table.add(user_idx as usize);
            
            // 안전 가드: 타겟 유저 세션이 실제로 존재하는지 비트 마스크 사전 검증
            if (*slot_ptr).user_id == 0 {
                return Err("존재하지 않거나 만료된 세션 슬롯입니다.");
            }

            // 2. 구조체 내의 `profile_hash_idx` 필드 주소를 원자적 제어가 가능한 형상으로 변환
            let atomic_profile_ptr = &(*slot_ptr).profile_hash_idx as *const u32 as *const AtomicU32;

            // 3. 락프리 원자적 스위칭 단행 (Atomic Store)
            // Release 순서 사상을 부여하여 이전 메모리 쓰기 작업(그림자 버퍼 채우기)이 
            // 커널 데이터 플레인(C)에 완벽하게 전파된 후 단번에 스왑되도록 하드웨어 버스를 제어합니다.
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
            let slot_ptr = self.raw_session_table.add(user_idx as usize);
            let atomic_mask_ptr = &(*slot_ptr).session_mask as *const u64 as *const core::sync::atomic::AtomicU64;

            // session_mask를 0x0000000000000000 으로 하드웨어 클록 단에서 증발시킴
            // 이 명령 즉시 bitwise_session_mux.c 는 if문 없이 해당 유저의 패킷을 XDP_DROP 처리합니다.
            (*atomic_mask_ptr).store(0x0000000000000000, Ordering::SeqCst);
        }

        Ok(())
    }
}
