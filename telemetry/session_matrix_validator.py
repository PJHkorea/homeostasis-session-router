# -*- coding: utf-8 -*-
# telemetry/session_matrix_validator.py

import numpy as np
import ctypes
from collections import deque
from typing import Dict, Tuple

class UserMetricsTensor(ctypes.Structure):
    """ 
    [★ 하드웨어 정렬: 하위 Rust ABI 및 CUDA 가속기 칩셋 구조와 1:1 완벽 호환 ABI Lock]
    기존의 4바이트 패딩은 총합 28B를 형성하여 가속기 벡터화 메모리 로딩 규격을 파괴했습니다.
    패딩을 정확히 8바이트(char * 8)로 확장하여 32바이트 하드웨어 스트라이드 배수를 완벽 수호합니다.
    """
    _fields_ = [
        ("user_id", ctypes.c_uint64),          # 8 Bytes (오프셋 0)
        ("rps", ctypes.c_float),               # 4 Bytes (오프셋 8)
        ("pps", ctypes.c_float),               # 4 Bytes (오프셋 12)
        ("payload_variance", ctypes.c_float),  # 4 Bytes (오프셋 16)
        ("error_rate", ctypes.c_float),        # 4 Bytes (오프셋 20)
        ("_pad", ctypes.c_char * 8)            # 8 Bytes (오프셋 24) -> 정확히 32바이트
    ]

class SessionMatrixValidator:
    def __init__(self, trace_window_size: int = 10, anomaly_threshold: float = 1e-4, max_tracked_users: int = 50000):
        """
        :param trace_window_size: 유저별 시계열 특징 상태를 추적할 윈도우 크기
        :param anomaly_threshold: 행렬식(Determinant) 붕괴 임계치 (0.0에 근사할수록 동기화된 봇 폭격)
        :param max_tracked_users: [★ 고도화] 자원 고갈 공격(OOM)을 무력화하기 위한 정적 유저 추적 한계선 바운더리
        """
        self.window_size = trace_window_size
        self.threshold = anomaly_threshold
        self.max_tracked_users = max_tracked_users
        
        # [★ 고도화: 공간 복잡도 O(1) 고정형 래티스 딕셔너리 전환]
        # 유저별 4대 특징 축 시계열 히스토리 버퍼
        self.user_timelines: Dict[int, deque] = {}
        
        # 가동 메모리 릭 폭발을 원천 제어하기 위한 LRU 축출용 인덱스 순서 레일
        self.user_lru_order: deque = deque()

    def push_metric_tensor(self, raw_tensor_ptr) -> Tuple[bool, float]:
        """
        Rust 프록시 단에서 무복사(Zero-copy)로 토스해준 유저 메트릭스 텐서를 낚아채어 검증합니다.
        """
        # 1. 0-Copy 포인터 캐스팅 및 데이터 인출 (패딩 수정으로 32B 정확 정렬 참조 보증)
        tensor = ctypes.cast(raw_tensor_ptr, ctypes.POINTER(UserMetricsTensor)).contents
        user_id = tensor.user_id
        
        # 하드웨어 버스 슬롯 데이터 다이렉트 벡터화 추출
        current_features = np.array([
            tensor.rps,
            tensor.pps,
            tensor.payload_variance,
            tensor.error_rate
        ], dtype=np.float32)

        # 2. [★ 고도화: L7 자원 고갈 공격 방어용 정적 공간 구속 인터록]
        if user_id not in self.user_timelines:
            # 신규 유저 진입 시 풀 상한선 도달 여부 사전 역산
            if len(self.user_timelines) >= self.max_tracked_users:
                # 가장 오래 활동이 없었던 공격 무리의 잔재 컨텍스트 메모리를 강제 축출(Evict)하여 
                # 파이썬 관제 영역의 힙 공간 메모리를 완벽하게 상수로 홀딩 동결시킵니다.
                oldest_user = self.user_lru_order.popleft()
                if oldest_user in self.user_timelines:
                    del self.user_timelines[oldest_user]
            
            # 리스트 대신 pop(0) 오버헤드가 정확히 0%인 고정 크기 순환 큐 deque로 공간 동결
            self.user_timelines[user_id] = deque(maxlen=self.window_size)
        else:
            # 기존 유저인 경우 LRU 갱신을 위해 순서 재배정 가드
            self.user_lru_order.remove(user_id)
            
        self.user_timelines[user_id].append(current_features)
        self.user_lru_order.append(user_id)
        
        if len(self.user_timelines[user_id]) < self.window_size:
            return True, 1.0 # 대수학적 데이터 가용 윈도우가 가득 찰 때까지는 정상 통과(XDP_PASS)

               # (1단계의 고정 크기 deque(maxlen=self.window_size) 구조 변경으로 인해,
        # 기존의 무거운 pop(0) 코드 라인은 자동으로 원천 거세 및 생략 처리되었습니다. 복잡도 O(1) 달성.)

        # 3. 4x4 공분산 행렬 생성 및 위상 붕괴 감지
        # 행렬의 축: [RPS, PPS, 가변페일로드분산, 에러율]
        # [★ 하드웨어 최적화: 가비지 컬렉터(GC) 부하를 없애는 연속 주소선 변환]
        matrix = np.ascontiguousarray(
            np.array(self.user_timelines[user_id], dtype=np.float32).T, 
            dtype=np.float32
        )  # Shape: (4, window_size)
        
        covariance_matrix = np.cov(matrix)  # Shape: (4, 4)
        
        # 4. 행렬식(Determinant) 계산을 통한 위상 공간 부피 측정
        # 정상 유저: 인간의 행동은 독립적이므로 4차원 공간의 부피(det)가 넓게 팽창함.
        # 매크로/봇: 기계적 알고리즘에 의해 4차원 특징 축들이 서로 강제 선형 종속(Singular)을 이룸.
        #           이로 인해 매 매니폴드 차원이 납작하게 짜부라지며 det 가 정확히 0.000000...으로 수렴함.
        try:
            det = np.linalg.det(covariance_matrix)
            
            # 수치 해석적 NaN 및 무한대(Inf) 전염 박멸 가드레일 집행
            if np.isnan(det) or np.isinf(det):
                det = 0.0
        except (np.linalg.LinAlgError, ValueError):
            det = 0.0  # 수치해석적 행렬 불안정 및 Singular 크래시 발생 시 붕괴(공격 상태)로 최종 확정

        # 5. 아노말리 판정 및 차단 시그널 제어 평면(Rust)으로 피딩
        if det < self.threshold:
            # 타겟 유저 세션 위상 붕괴 검출 완료 (어뷰징 매크로/세션 하이재킹 확정)
            # 메모리 릭 방지를 위해 윈도우 풀 및 LRU 레일에서 해당 유저의 흔적을 영구 축출합니다.
            self._evict_user_history(user_id)
            
            # False 리턴 즉시 Rust 마스터 사령탑(`main.rs`)이 `emergency_gating_lock` 비트 락을 단행합니다.
            return False, float(det)

        return True, float(det)

    def _evict_user_history(self, user_id: int):
        """ 위상 공간 붕괴가 확정된 악성 군집 유저의 관제 자원을 청정 제로 베이스로 축출 """
        if user_id in self.user_timelines:
            del self.user_timelines[user_id]
            
        # [★ 고도화: LRU 가드레일 오염 방지 인터록]
        # 축출된 유저가 LRU 링 버퍼 내에 상주하여 발생하는 가상 인덱스 미스매치 결함을 원천 소거합니다.
        try:
            self.user_lru_order.remove(user_id)
        except ValueError:
            pass

