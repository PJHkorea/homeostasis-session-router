# =========================================================================
# [Sovereign Buffer Cache - 5th-Gen Ingress Kernel Driver Makefile]
# =========================================================================

# 1. 🎛️ 하드웨어 컴파일러 및 최적화 엔진 론치 패드 정의
CLANG    ?= clang
LLC      ?= llc
LLVM_STRIP ?= llvm-strip

# 리눅스 커널 표준 아키텍처 타겟 강제 지정 (eBPF 64비트 엔디언 독립성 확보)
ARCH     := ebpftarget

# 2. 🛡️ 시스템 경계 플래그 인터록 (`config/system_bounds.toml`과 바이트 스펙 일치)
# max_entries 1,000만 슬롯 선점을 커널 C 헤더 매크로 상수로 컴파일 타임에 하드라킹(Hard-locking)합니다.
MAX_USER_SLOTS ?= 10000000

# 3. 💎 [하드웨어 최적화: Verifier 박멸을 위한 클랭 최적화 플래그 체인]
# -O2: LLVM 레지스터 할당 최적화 및 불필요한 스택 복사 거세
# -g: CO-RE(Compile Once - Run Everywhere) 작동을 위한 BTF 디버깅 심볼 주입 (필수)
# -D__BPF_TRACING__: 커널 하이재킹 전용 내부 매크로 활성화
CFLAGS := -g -O2 -Wall \
          -target bpf \
          -D__TARGET_ARCH_$(ARCH) \
          -D__BPF_TRACING__ \
          -DMAX_USER_SLOTS=$(MAX_USER_SLOTS) \
          -I/usr/include \
          -I/usr/include/bpf \
          -I.

# 4. 타겟 아티팩트 비트 바인딩 명세
TARGET   := bitwise_session_mux
BPF_OBJ  := $(TARGET).o

.PHONY: all clean check_env load_g 가드

all: check_env $(BPF_OBJ)
	@echo "[+] 성공: 커널 데이터 플레인 가속 바이트코드 빌드 완료 -> $(BPF_OBJ)"
	@echo "[+] 정적 공간 복잡도 수호 마진: $(MAX_USER_SLOTS) Slots (Hard-locked)"

# 5. 🔥 [기계어 직역 파이프라인]: C 코드를 완벽한 무분기 eBPF 바이트코드로 합성
# llvm-strip -g 명령어를 통해 커널 적재에 불필요한 임시 기호만 도려내고 CO-RE용 BTF 섹션은 완벽 보존합니다.
$(BPF_OBJ): $(TARGET).c session_maps.h
	@echo "[*] 컴파일 중: Clang 레지스터 파이프라인 최적화 전개..."
	$(CLANG) $(CFLAGS) -c $< -o $@
	@echo "[*] 최적화 튜닝: 불필요 기호 제거 및 실리콘 레벨 ABI 무결성 압착..."
	$(LLVM_STRIP) -g $@

# 6. 🚨 빌드 인프라 환경 사전 실시간 단언(Assert) 가드레일
check_env:
	@command -v $(CLANG) >/dev/null 2>&1 || (eprintln "[-] 치명적 결함: Clang 컴파일러가 호스트에 없습니다!"; exit 1)
	@command -v $(LLVM_STRIP) >/dev/null 2>&1 || (eprintln "[-] 치명적 결함: llvm-strip 가 존재하지 않습니다!"; exit 1)
	@if [ ! -f /usr/include/vmlinux.h ] && [ ! -f ./vmlinux.h ]; then \
		echo "[⚠️ WARNING] vmlinux.h 커널 헤더 조각이 누락되었습니다. CO-RE 안정성을 위해 빌드 전 배치를 추천합니다."; \
	fi

# 7. 인프라 청정화 롤백 엔진
clean:
	@echo "[*] 청정화: 임시 커널 바이트코드 링거링 자원 소거..."
	rm -f $(BPF_OBJ)
	@echo "[+] 완료: 제로 베이스 청정 상태 복구 성공."
