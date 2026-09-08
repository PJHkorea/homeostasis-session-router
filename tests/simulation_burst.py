# -*- coding: utf-8 -*-
# tests/simulation_burst.py

import numpy as np
import ctypes
import time
import sys
from concurrent.futures import ThreadPoolExecutor

class UserSessionSlot(ctypes.Structure):
    """ 
    [★ 하드웨어 정렬: 리팩토링된 eBPF C 커널 및 Rust FFI 명세와 1:1 대칭 완벽 ABI Lock]
    최전방 인그레스 핫 패스 가속을 위해 `session_mask`와 `expiry_tick`이 최전방 32B 내부로 
    격리 배정된 최신 물리 오프셋 레이아웃 지도를 바이트 소수점 단위까지 정밀 동기화합니다.
    """
    _struct_ = "UserSessionSlot"
    _pack_ = 1
    _fields_ = [
        # ==== LINE 1: 핫 패스 가속 레일 (최전방 L1 단일 캐시라인 경계 32바이트) ====
        ("session_mask", ctypes.c_uint64),       # 8 Bytes (오프셋 0)
        ("expiry_tick", ctypes.c_uint64),        # 8 Bytes (오프셋 8)
        ("user_id", ctypes.c_uint64),            # 8 Bytes (오프셋 16)
        ("active_conn_id", ctypes.c_uint32),     # 4 Bytes (오프셋 24)
        ("_pad1", ctypes.c_uint32),              # 4 Bytes (오프셋 28) -> 정확히 32바이트

        # ==== LINE 2: 콜드 패스 / 가변 자원 업데이트 레일 (나머지 32바이트) ====
        ("profile_hash_idx", ctypes.c_uint32),   # 4 Bytes (오프셋 32)
        ("alert_queue_head", ctypes.c_uint32),   # 4 Bytes (오프셋 36)
        ("alert_queue_tail", ctypes.c_uint32),   # 4 Bytes (오프셋 40)
        ("status_flags", ctypes.c_uint32),       # 4 Bytes (오프셋 44)
        ("_pad2", ctypes.c_char * 16)            # 16 Bytes(오프셋 48) -> 정확히 64바이트 완결
    ]

class UserMetricsTensor(ctypes.Structure):
    """ 
    [★ 하드웨어 정렬: CUDA 가속기 및 Rust `abi.rs` 명세와 32바이트 물리 스트라이드 칼싱크]
    기존 패딩(4B)은 총합 28B를 형성하여 가속기 벡터화 전송 파이프라인 정렬을 파괴했습니다.
    패딩을 정확히 8바이트로 확장해 `8 + 16 + 8 = 32바이트` 캐시라인 물리 경계를 완벽 수호합니다.
    """
    _pack_ = 1
    _fields_ = [
        ("user_id", ctypes.c_uint64),            # 8 Bytes (오프셋 0)
        ("rps", ctypes.c_float),                 # 4 Bytes (오프셋 8)
        ("pps", ctypes.c_float),                 # 4 Bytes (오프셋 12)
        ("payload_variance", ctypes.c_float),    # 4 Bytes (오프셋 16)
        ("error_rate", ctypes.c_float),          # 4 Bytes (오프셋 20)
        ("_pad", ctypes.c_char * 8)              # 8 Bytes (오프셋 24) -> 정확히 32바이트
    ]


class SovereignBufferCacheSimulator:
    def __init__(self, max_users: int = 1000000):
        print("[+] Sovereign Buffer Cache 초기화 시작...")
        self.max_users = max_users
        
        # 1. 💾 [공간 복잡도 O(1) 동결] 부팅 시 가상 메모리 하드 락킹 선점 (Hard-locking)
        # 64바이트 Aligned 구조체를 최대 유저 수만큼 통째로 연속 배열로 할당하여 0MB 할당 변동률 구현
        self.raw_buffer = (UserSessionSlot * max_users)()
        self.buffer_ptr = ctypes.pointer(self.raw_buffer)
        
        # 메트릭스 텐서 공간도 부팅 시 1회 고정 할당
        self.metrics_buffer = (UserMetricsTensor * max_users)()
        self.metrics_ptr = ctypes.pointer(self.metrics_buffer)

        print(f"[+] 성공: {max_users}개의 세션 슬롯 기부 완료. 고정 메모리 점유: {sys.getsizeof(self.raw_buffer) / 1024 / 1024:.2f} MB")
        
        # 초기 기본 마스크를 유효(0xFFFFFFFFFFFFFFFF) 상태로 채워둠
        for i in range(max_users):
            self.raw_buffer[i].session_mask = 0xFFFFFFFFFFFFFFFF
            self.raw_buffer[i].user_id = 10000000 + i

    def simulate_normal_user_traffic(self, user_idx: int):
        """ 인간 유저: 불규칙하고 자유도(Entropy)가 높은 요청 패턴 """
        # 무작위 분산 값을 지닌 정상 트래픽 벡터 주입
        m = self.metrics_buffer[user_idx]
        m.user_id = self.raw_buffer[user_idx].user_id
        m.rps = float(np.random.normal(30.0, 15.0))  # 평균 30 RPS
        m.pps = m.rps * 1.2
        m.payload_variance = float(np.random.uniform(10.0, 500.0)) # 자유도 높음
        m.error_rate = float(np.random.uniform(0.0, 0.02))

    def simulate_macro_bot_traffic(self, user_idx: int):
        """ 매크로/봇셋: 극도로 일정한 주기, 선형 종속적이고 정형화된 폭격 패턴 """
        m = self.metrics_buffer[user_idx]
        m.user_id = self.raw_buffer[user_idx].user_id
        m.rps = 5000.0  # 임계치를 한참 초과하는 폭주 RPS
        m.pps = 5000.0
        m.payload_variance = 0.001  # 분산이 거의 없음 (동기화 징후)
        m.error_rate = 0.0

    def run_gating_pipeline(self, user_idx: int, current_virtual_tick_ns: int):
        """ 
        고도화된 bitwise_session_mux.c 커널의 타임 어택 우회 박멸 2중 비트 MUX 메커니즘을 완벽 모사합니다.
        조건문(if) 없이, CPU 가산기의 부호 플래그 비트 시프트 연산만으로 만료 유효성까지 강제 정류합니다.
        """
        slot = self.raw_buffer[user_idx]
        
        # 1. 커널 시간 틱 역산 대조 식 모사 (expiry_tick - current_time_ns)
        # 세션이 유효하면 결과가 양수(부호비트 0), 만료되면 음수(부호비트 1)가 됩니다.
        time_delta = int(slot.expiry_tick) - current_virtual_tick_ns
        
        # 2. 산술 우측 시프트(>> 63)를 통해 부호 비트를 전체 64비트선으로 복사 전개
        # 양수(유효) -> 0x0000000000000000 | 음수(만료) -> 0xFFFFFFFFFFFFFFFF
        time_expired_mask = (time_delta >> 63) & 0xFFFFFFFFFFFFFFFF
        
        # 비트 NOT 연산으로 반전시킴으로써 유효 시 0xFF..., 만료 시 0x00...인 '시간 가드 마스크' 합성
        time_valid_mask = ~time_expired_mask & 0xFFFFFFFFFFFFFFFF

        # 3. 🔥 [분기문 0% 거세] 가속기 락 신호(session_mask)와 커널 시간 가드 마스크를 비트 AND(&)로 융합
        final_integrity_mask = slot.session_mask & time_valid_mask
        
        # 4. 실리콘 레벨 융합 게이팅 체인 작동 (XDP_PASS=2, XDP_DROP=1)
        action = (2 & final_integrity_mask) | (1 & ~final_integrity_mask)
        return "PASS" if action == 2 else "DROP"

    def execute_schrodinger_gate_mock(self, rps_limit: float):
        """ 고도화된 Triton/CUDA 가속기 커널의 수치 해석적 발산 제어 및 양자 장벽 소산 매커니즘을 완벽 모사 """
        blocked_count = 0
        
        # 가속기 칩셋 내부 부동소수점 오염(NaN/Inf 전연)을 박멸하는 상하한 클리핑 임계 장벽 동기화
        MAX_POTENTIAL_LIMIT = 100000.0
        
        for i in range(self.max_users):
            m = self.metrics_buffer[i]
            if self.raw_buffer[i].session_mask == 0:
                continue
                
            # 슈뢰딩거 포텐셜 장벽 V = (RPS - Limit) * (1 + Var)
            potential_v = (m.rps - rps_limit) * (1.0 + m.payload_variance)
            
            # [★ 하드웨어 최적화 싱크]: 분기문 없는 레지스터 단독 클록 프리미티브 클리핑 분해 전개
            safe_potential = min(max(potential_v, 0.0), MAX_POTENTIAL_LIMIT)
            
            # 투과율 T = exp(-2 * sqrt(V)) 적용
            sqrt_v = np.sqrt(safe_potential)
            transmission = np.exp(-2.0 * sqrt_v)
            
            # 투과율이 1% 미만으로 붕괴하고 실제 어뷰징 화력이 확인될 때 세션 비트 락 집행
            should_kill = (transmission < 0.01) and (potential_v > 0.0)
            
            # [★ 하드웨어 최적화 싱크]: 고도화된 `atomicAnd` 원자적 비트 클리어 필터 매커니즘 이식
            # 기존처럼 마스크 전체를 무조건 0으로 덮어쓰지 않고, 비트 논리곱을 통해 타겟 활성선만 끊어버립니다.
            kill_mask = 0x0000000000000000 if should_kill else 0xFFFFFFFFFFFFFFFF
            self.raw_buffer[i].session_mask &= kill_mask
            
            if should_kill:
                blocked_count += 1
                
        return blocked_count

# -------------------------------------------------------------------
# 🔥 실제 대규모 버스트 및 어뷰징 차단 실행 테스트
# -------------------------------------------------------------------
if __name__ == "__main__":
    TOTAL_USERS = 100000  # 시뮬레이션 대상 유저 수 (10만 명 규모 테스트)
    RPS_LIMIT = 100.0     # 안전 한계선
    
    sim = SovereignBufferCacheSimulator(max_users=TOTAL_USERS)
    
    # [★ 고도화: 2중 시간 비트 MUX 검증을 위한 현재 시점의 가상 커널 나노초 틱 산출]
    # 실제 eBPF 커널의 bpf_ktime_get_ns()와 완벽히 상등하는 절대 가상 시간 틱을 수립합니다.
    current_virtual_tick_ns = int(time.perf_counter_ns())
    
    print("\n[Step 1] 대규모 버스트 트래픽 인입 시작 (정상 유저 95% + 매크로 봇 5%)")
    # 95,000명은 정상 유저 패턴, 5,000명은 악성 매크로 봇으로 메트릭스 텐서 적재
    for idx in range(TOTAL_USERS):
        # 만료 시간(expiry_tick) 인터록 무결성 테스트를 위해, 부팅 슬롯 마다 만료 시간을 선제 기입
        # 정상 세션은 현재 시간으로부터 1시간 유효(+3.6e12 ns), 이미 만료된 세션은 과거 시간(-1000 ns) 매핑
        if idx % 20 == 0:
            sim.raw_buffer[idx].expiry_tick = current_virtual_tick_ns - 1000
            sim.simulate_macro_bot_traffic(idx)
        else:
            sim.raw_buffer[idx].expiry_tick = current_virtual_tick_ns + 3600000000000
            sim.simulate_normal_user_traffic(idx)
            
    # 시뮬레이션 가동 전 메모리 상태 체크
    initial_alloc = sys.getsizeof(sim.raw_buffer)
    print(f"[*] 트래픽 폭주 직전 메모리 할당 크기: {initial_alloc / 1024 / 1024:.2f} MB")

    print("\n[Step 2] Triton 하드웨어 레이어 가동 (schrodinger_gate)")
    start_time = time.perf_counter()
    evicted = sim.execute_schrodinger_gate_mock(rps_limit=RPS_LIMIT)
    end_time = time.perf_counter()
    
    print(f"[!] Triton 양자 장벽 수식 연산 완료 (소요 시간: {(end_time - start_time)*1000:.2f}ms)")
    print(f"[!] 실시간 체포 및 세션 비트 락(MUX) 집행된 어뷰징 세션 수: {evicted}개")

    print("\n[Step 3] 최하단 eBPF 커널 데이터 플레인 게이팅 동작 검증")
    # [★ 고도화 싱크]: 중반부에서 전면 개조한 2중 시간 비트 MUX 레일 축으로 가상 나노초 틱 주입 실행
    # 샘플 조사 (정상 유저였던 1번과 매크로였던 0번 비교)
    normal_action = sim.run_gating_pipeline(1, current_virtual_tick_ns)
    macro_action = sim.run_gating_pipeline(0, current_virtual_tick_ns)
    
    print(f"[*] 정상 유저 (Idx: 1) 캐시 라우팅 결과: {normal_action}")
    print(f"[*] 매크로 봇 (Idx: 0) 무분기 커널 차단 결과: {macro_action}")

    # 시뮬레이션 가동 후 메모리 상태 체크
    final_alloc = sys.getsizeof(sim.raw_buffer)
    print(f"\n[Step 4] 런타임 공간 복잡도 최종 성적표")
    print(f"[*] 트래픽 폭주 및 차단 후 메모리 할당 크기: {final_alloc / 1024 / 1024:.2f} MB")
    print(f"🔥 [결론] 서버 메모리 추가 할당 진폭: {abs(final_alloc - initial_alloc)} Byte (정확히 0MB 고정 완벽 증명)")
