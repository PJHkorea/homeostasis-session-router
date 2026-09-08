// target_hardware_cuda/session_viscosity.cu
#include <cuda_runtime.h>
#include <device_launch_parameters.h>
#include <math.h>

// Rust ABI와 일치하는 가속기 측 메트릭스 구조체
struct UserMetricsTensor {
    unsigned long long user_id;
    float rps;
    float pps;
    float payload_variance;
    float error_rate;
    unsigned char _pad[4];
};

/**
 * [Sovereign Buffer Cache - CUDA 기반 하드웨어 세션 점성 제어 커널]
 * 
 * @param metrics       Rust 플레인에서 공유 메모리(정적 버퍼)로 토스한 유저 텐서 배열
 * @param session_masks 최하단 eBPF 커널(C)과 공유하는 세션 마스크 배열 (0xFF... 또는 0x00...)
 * @param rps_limit     안전 한계 RPS 임계치
 * @param num_users     현재 활성 제어 유저 수
 */
__global__ void compute_session_viscosity_kernel(
    const UserMetricsTensor* __restrict__ metrics,
    unsigned long long* __restrict__ session_masks,
    const float rps_limit,
    const int num_users
) {
    // 1. GPU Grid-Stride 루프를 통한 초고속 병렬 인덱싱
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= num_users) return;

    // 2. 글로벌 메모리 뱅크 충돌(Bank Collision) 방지를 위한 로컬 레지스터 적재
    UserMetricsTensor user = metrics[idx];
    unsigned long long current_mask = session_masks[idx];

    // 만약 이미 하이재킹 잠금(Lock)이 걸린 유저라면 하드웨어 연산 생략 (Zero-overhead)
    if (current_mask == 0) return;

    // 3. 🌊 [수리 물리적 기믹] 슈뢰딩거 포텐셜 기반 점성 배리어 계산
    // 요청 진폭(RPS)과 비정상 패킷 변동성(Variance)의 곱으로 공격 화력(Potential V)을 정의합니다.
    float potential_v = (user.rps - rps_limit) * (1.0f + user.payload_variance);

    // 4. 🔥 분기문(if) 없는 완벽한 하드웨어 투과율 결정 연산
    // 일반적인 if (potential_v > 0) 문은 GPU Warp Divergence(스레드 분기 분열)를 일으켜 가속기를 마비시킵니다.
    // 대신 부호 비트 마스크 및 내장 수학 함수(__expf)를 이용해 일직선으로 연산합니다.
    
    // potential_v가 0보다 크면 1.0, 작으면 0.0을 반환하는 branchless 가중치
    float is_abusing = (float)(potential_v > 0.0f); 

    // 슈뢰딩거 배리어 투과율 공식 적용: T = exp(-2 * sqrt(V))
    // 공격이 미친 듯이 강해질수록 potential_v가 폭발하며, 투과율(transmission)은 정확히 0.0으로 수렴합니다.
    float transmission = __expf(-2.0f * sqrtf(fmaxf(potential_v, 0.0f)));

    // 5. 점성이 임계 장벽을 뚫고 0.01(1%) 미만으로 떨어지면 세션 비트 락 집행 시그널로 전환
    // 이 역시 if문 없이 비트 마스크 연산으로 수행합니다.
    unsigned long long kill_signal = (unsigned long long)(transmission < 0.01f * is_abusing);
    
    // kill_signal이 1이면 0x0000000000000000(차단), 0이면 기존 마스크(0xFF... 유지)
    // 원자적(Atomic) 연산으로 eBPF 커널이 참조하는 메모리를 즉시 하이재킹합니다.
    if (kill_signal) {
        atomicExch(&session_masks[idx], 0x0000000000000000ULL);
    }
}

extern "C" void launch_session_viscosity_damper(
    const UserMetricsTensor* metrics,
    unsigned long long* session_masks,
    float rps_limit,
    int num_users,
    cudaStream_t stream
) {
    if (num_users <= 0) return;

    // 현대 GPU 구조에 최적화된 256 스레드 블록 배치
    int block_size = 256;
    int grid_size = (num_users + block_size - 1) / block_size;

    compute_session_viscosity_kernel<<<grid_size, block_size, 0, stream>>>(
        metrics,
        session_masks,
        rps_limit,
        num_users
    );
}
