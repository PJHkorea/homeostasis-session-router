// target_proxy_rust/src/abi.rs

/// [Sovereign Buffer Cache - 핵심 유저 세션 고정 버퍼 디자인]
/// 
/// 하드웨어 캐시 라인(64 바이트) 크기에 완벽히 정렬(Align)하여, 
/// 이 구조체를 읽고 쓸 때 CPU 캐시 미스 및 캐시 라인 오염(False Sharing)을 원천 차단합니다.
/// C 언어의 `struct` 및 eBPF 커널 내부 맵과 1:1 메모리 호환성을 가집니다.
#[repr(C, align(64))]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UserSessionSlot {
    // ---------------- [1번 캐시라인 패스: 세션 검증 가드 (32바이트)] ----------------
    
    /// 유저 고유 식별자 (0이면 빈 슬롯)
    pub user_id: u64,
    
    /// 세션 만료 타임스탬프 (에포크 밀리초 또는 커널 틱)
    pub expiry_tick: u64,
    
    /// 세션 검증 게이트용 비트 마스크
    /// 유효 세션: 0xFFFFFFFFFFFFFFFF, 만료/차단 세션: 0x0000000000000000
    /// bitwise_session_mux.c 에서 조건문(if) 없이 AND 연산용으로 사용됨
    pub session_mask: u64,
    
    /// 현재 활성화된 네트워크 커널 소켓/커넥션 매핑 ID
    pub active_conn_id: u32,
    
    /// 32바이트 경계를 맞추기 위한 정적 패딩
    pub _pad1: u32,

    // ---------------- [2번 캐시라인 패스: 프로필 및 데이터 라우팅 (32바이트)] ----------------
    
    /// 하드 락킹된 고정 버퍼 내 프로필 이미지의 오프셋 주소 고유 인덱스
    /// 질문하셨던 프로필 수정 발생 시, Rust 단에서 이 인덱스를 원자적으로 Swap 합니다.
    pub profile_hash_idx: u32,
    
    /// 알림(Notification) 락프리 링버퍼의 Head 포인터 오프셋
    pub alert_queue_head: u32,
    
    /// 알림(Notification) 락프리 링버퍼의 Tail 포인터 오프셋
    pub alert_queue_tail: u32,
    
    /// 유저 상태 플래그 (예: 0=정상, 1=어뮤징 의심, 2=영구차단)
    pub status_flags: u32,
    
    /// 64바이트 전체 크기를 칼정렬하기 위한 최종 나머지 패딩
    pub _pad2: [u8; 16],
}

/// 섀도우 엔진(telemetry) 및 가속기(CUDA)로 무복사(Zero-copy) 토스하기 위한 
/// 4대 유저 접속 특징 벡터 텐서 구조체 (4x4 공분산 행렬의 기초 데이터)
#[repr(C, align(32))]
#[derive(Debug, Copy, Clone)]
pub struct UserMetricsTensor {
    pub user_id: u64,
    pub rps: f32,                // 초당 요청 수 (Request Per Second)
    pub pps: f32,                // 초당 패킷 수 (Packet Per Second)
    pub payload_variance: f32,   // 페이로드 크기의 분산 값 (매크로 봇넷 탐지용)
    pub error_rate: f32,         // 인증 실패 및 에러 발생률
    pub _pad: [u8; 4],           // 32바이트 정렬 패딩
}

impl UserSessionSlot {
    /// 빈 세션 슬롯 생성자 (부팅 시 가상 메모리 하드 락킹 초기화용)
    pub fn empty() -> Self {
        Self {
            user_id: 0,
            expiry_tick: 0,
            session_mask: 0x0000000000000000,
            active_conn_id: 0,
            _pad1: 0,
            profile_hash_idx: 0,
            alert_queue_head: 0,
            alert_queue_tail: 0,
            status_flags: 0,
            _pad2: [0; 16],
        }
    }
}
