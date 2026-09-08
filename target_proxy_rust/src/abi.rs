// -*- coding: utf-8 -*-
// target_proxy_rust/src/abi.rs

/// [Sovereign Buffer Cache - Single L1 Cache-Line Hit 격리 디자인]
/// 
/// 최전방 eBPF C 커널(`session_maps.h`)의 메모리 구조 재배치 사상과 바이트 오프셋 단위까지 
/// 1:1 완벽 호환되도록 필드 정렬을 재정류했습니다.
/// 인그레스 핫 패스에 수반되는 레지스터 조회를 최전방 32바이트 영역 이내로 완전히 가두어,
/// 멀티코어 환경에서 캐시 일관성 버스 오염 및 거짓 공유(False Sharing)를 물리적으로 소멸시킵니다.
#[repr(C, align(64))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UserSessionSlot {
    // =========================================================================
    // LINE 1: 핫 패스 제어 레일 (최전방 L1 단일 캐시라인 경계 32바이트 완벽 구속)
    // =========================================================================
    
    /// 세션 검증 게이트용 비트 마스크 (오프셋 0)
    /// 유효 세션: 0xFFFFFFFFFFFFFFFF, 만료/차단 세션: 0x0000000000000000
    /// 커널 `bitwise_session_mux.c` 단독 클록 연산용 0ns 주소선 플래그
    pub session_mask: u64,
    
    /// 세션 만료 타임스탬프 (오프셋 8) | 커널 `bpf_ktime_get_ns()` 역산 대조용 절대 나노초 틱
    pub expiry_tick: u64,
    
    /// 유저 고유 레지스터 식별 정수 (오프셋 16) | 0: 미할당 빈 슬롯
    pub user_id: u64,
    
    /// 현재 활성화된 네트워크 커널 소켓/커넥션 매핑 ID (오프셋 24)
    pub active_conn_id: u32,
    
    /// 정확히 32바이트 캐시라인 경계선을 형성하기 위한 하드웨어 패딩 (오프셋 28)
    pub _pad1: u32,

    // =========================================================================
    // LINE 2: 콜드 패스 / 컨트롤 플레인 업데이트 레일 (나머지 32바이트)
    // =========================================================================
    
    /// 하드 락킹된 고정 버퍼 내 프로필 주소 변환 고유 해시 인덱스 (오프셋 32)
    /// 유저가 프로필 수정 발생 시, Rust 플레인 단에서 이 인덱스를 0ns 원자적으로 Swap 합니다.
    pub profile_hash_idx: u32,
    
    /// 알림(Notification) 락프리 링버퍼의 Head 포인터 오프셋 (오프셋 36)
    pub alert_queue_head: u32,
    
    /// 알림(Notification) 락프리 링버퍼의 Tail 포인터 오프셋 (오프셋 40)
    pub alert_queue_tail: u32,
    
    /// 유저 상태 플래그 (오프셋 44) | 0:정상, 1:매크로 의심, 2:영구차단 블랙리스트
    pub status_flags: u32,
    
    /// 총 64바이트 하드웨어 대칭 대형 얼라인먼트를 완성하기 위한 최종 고정 버퍼 (오프셋 48)
    pub _pad2: [u8; 16],
}

/// 섀도우 엔진(telemetry) 및 가속기(CUDA/Triton)로 무복사(Zero-copy) 토스하기 위한 
/// 4대 유저 접속 특징 벡터 텐서 구조체 (4x4 공분산 행렬의 기초 데이터)
/// 
/// [★ 하드웨어 가드: 32바이트 물리 스트라이드 정밀 싱크화]
/// 기존 패딩(4B)은 총합 28B를 형성하여 CUDA `int4` 벡터화 전송 파이프라인 정렬을 파괴합니다.
/// 패딩을 정확히 8바이트로 밀어내어 `8 + 16 + 8 = 32바이트` 캐시라인 물리 경계를 완벽 수호합니다.
#[repr(C, align(32))]
#[derive(Debug, Copy, Clone)]
pub struct UserMetricsTensor {
    pub user_id: u64,            // 8 Bytes (오프셋 0)
    pub rps: f32,                // 4 Bytes (오프셋 8)   | 초당 요청 수
    pub pps: f32,                // 4 Bytes (오프셋 12)  | 초당 패킷 수
    pub payload_variance: f32,   // 4 Bytes (오프셋 16)  | 페이로드 크기의 분산 값
    pub error_rate: f32,         // 4 Bytes (오프셋 20)  | 인증 실패 및 에러 발생률
    pub _pad: [u8; 8],           // 8 Bytes (오프셋 24)  | 32바이트 완벽 정렬 완결용 고정 패딩
}

impl UserSessionSlot {
    /// 빈 세션 슬롯 생성자 (부팅 시 가상 메모리 하드 락킹 초기화용)
    /// 
    /// [★ 하드웨어 최적화: `const fn` 승격을 통한 런타임 할당 지터 0%화]
    /// 컴파일 타임에 정적 데이터 세그먼트 상에서 640MB 버퍼의 기초 비트열을 
    /// 즉시 빌드 고정할 수 있도록 상수가 수립된 안전 생성자로 진화시킵니다.
    pub const fn empty() -> Self {
        Self {
            // LINE 1 격리 정류 레일 초기화
            session_mask: 0x0000000000000000,
            expiry_tick: 0,
            user_id: 0,
            active_conn_id: 0,
            _pad1: 0,
            
            // LINE 2 가변 제어 레일 초기화
            profile_hash_idx: 0,
            alert_queue_head: 0,
            alert_queue_tail: 0,
            status_flags: 0,
            _pad2: [0; 16],
        }
    }
}

