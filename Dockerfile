# =========================================================================
# [Sovereign Buffer Cache - 5th-Gen Cross-Domain Ingress Compiler Dockerfile]
# =========================================================================

# -------------------------------------------------------------------------
# STAGE 1: 리눅스 최하단 eBPF/XDP 커널 드라이버 및 BTF 심볼 빌드 레이어
# -------------------------------------------------------------------------
FROM ubuntu:24.04 AS kernel-builder

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y \
    clang \
    llvm \
    libbpf-dev \
    linux-headers-generic \
    make \
    git \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /build/kernel
COPY target_kernel_xdp/ .
COPY config/system_bounds.toml /build/config/

# system_bounds.toml에서 하드라킹 슬롯 상수를 역산하여 매크로 컴파일 강제 집행
RUN MAX_USER_SLOTS=$(awk -F'=' '/max_user_slots/{gsub(/[[:space:]]/,"",$2); print $2}' /build/config/system_bounds.toml) && \
    make MAX_USER_SLOTS=${MAX_USER_SLOTS} all

# -------------------------------------------------------------------------
# STAGE 2: 엔비디아 SM 아키텍처 점성 제어 CUDA/Triton 컴파일 레이어
# -------------------------------------------------------------------------
FROM nvidia/cuda:12.4.1-devel-ubuntu24.04 AS accelerator-builder

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y \
    python3-pip \
    python3-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Triton 가속 컴파일 파이프라인 종속성 이식
RUN pip3 install --break-system-packages triton==3.0.0 numpy==1.26.4 mmh3==4.1.0

WORKDIR /build/cuda
COPY target_hardware_cuda/ .

# nvcc 컴파일러를 가동하여 뱅크 충돌 0% 고속 PTX 및 가속 바이너리 추출
RUN nvcc -O3 -Xcompiler -march=native --ptx session_viscosity.cu -o session_viscosity.ptx

# -------------------------------------------------------------------------
# STAGE 3: FFI 경계를 무력화하는 Rust 마스터 오케스트레이터 빌드 레이어
# -------------------------------------------------------------------------
FROM rust:1.80-slim AS rust-orchestrator

WORKDIR /build/rust
COPY target_proxy_rust/ .
COPY --from=kernel-builder /build/kernel/ /build/kernel/
COPY --from=accelerator-builder /build/cuda/ /build/cuda/

# 링크 타임 전역 최적화(LTO Fat) 가드가 심어진 청정 정적 기계어 덤프
RUN cargo build --release

# -------------------------------------------------------------------------
# STAGE 4: 실전 가동 프로덕션 론치 이미지 마감 (최소 공간 복잡도 구속)
# -------------------------------------------------------------------------
FROM nvidia/cuda:12.4.1-runtime-ubuntu24.04 AS production-runtime

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y \
    libbpf1 \
    python3 \
    python3-pip \
    iproute2 \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# 파이썬 대수학 관제탑 및 어댑터 구동을 위한 무결성 런타임 이식
RUN pip3 install --break-system-packages numpy==1.26.4 mmh3==4.1.0 pynvml==11.5.0

WORKDIR /app
COPY config/ /app/config/
COPY telemetry/ /app/telemetry/
COPY adapters/ /app/adapters/

# 앞선 빌드 스테이지에서 정제된 정적 바이너리와 eBPF 바이트코드 포인터만 수량 기부
COPY --from=rust-orchestrator /build/rust/target/release/session_router_daemon /app/
COPY --from=kernel-builder /build/kernel/bitwise_session_mux.o /app/kernel/
COPY --from=accelerator-builder /build/cuda/session_viscosity.ptx /app/cuda/

# 호스트 OS의 실리콘 네트워크 카드 드라이버로 돌입하기 위한 진입점 대기 고정
ENTRYPOINT ["/app/session_router_daemon"]
