# telemetry/session_matrix_validator.py
import numpy as np
import ctypes
from typing import Dict, Tuple

class UserMetricsTensor(ctypes.Structure):
    """ target_proxy_rust/src/abi.rs 의 UserMetricsTensor 구조체와 1:1 매핑 """
    _fields_ = [
        ("user_id", ctypes.c_uint64),
        ("rps", ctypes.c_float),
        ("pps", ctypes.c_float),
        ("payload_variance", ctypes.c_float),
        ("error_rate", ctypes.c_float),
        ("_pad", ctypes.c_char * 4)
    ]

class SessionMatrixValidator:
    def __init__(self, trace_window_size: int = 10, anomaly_threshold: float = 1e-4):
        """
        :param trace_window_size: 유저별 시계열 특징 상태를 추적할 윈도우 크기
        :param anomaly_threshold: 행렬식(Determinant) 붕괴 임계치 (0.0에 근사할수록 동기화된 봇 폭격)
        """
        self.window_size = trace_window_size
        self.threshold = anomaly_threshold
        
        # 유저별 4대 특징 축(RPS, PPS, 분산, 에러율) 시계열 히스토리 버퍼
        self.user_timelines: Dict[int, list] = {}

    def push_metric_tensor(self, raw_tensor_ptr) -> Tuple[bool, float]:
        """
        Rust 프록시 단에서 무복사(Zero-copy)로 토스해준 유저 메트릭스 텐서를 낚아채어 검증합니다.
        """
        # 1. 0-Copy 포인터 캐스팅 및 데이터 인출
        tensor = ctypes.cast(raw_tensor_ptr, ctypes.POINTER(UserMetricsTensor)).contents
        user_id = tensor.user_id
        
        current_features = np.array([
            tensor.rps,
            tensor.pps,
            tensor.payload_variance,
            tensor.error_rate
        ], dtype=np.float32)

        # 2. 유저별 시계열 상태 윈도우 업데이트
        if user_id not in self.user_timelines:
            self.user_timelines[user_id] = []
        
        self.user_timelines[user_id].append(current_features)
        
        if len(self.user_timelines[user_id]) < self.window_size:
            return True, 1.0 # 데이터가 쌓일 때까지는 정상 통과(XDP_PASS 유지)

        # 윈도우 크기 유지
        self.user_timelines[user_id].pop(0)
        
        # 3. 4x4 공분산 행렬 생성 및 위상 붕괴 감지
        # 행렬의 축: [RPS, PPS, 가변페일로드분산, 에러율]
        matrix = np.array(self.user_timelines[user_id], dtype=np.float32).T # Shape: (4, window_size)
        covariance_matrix = np.cov(matrix) # Shape: (4, 4)
        
        # 4. 행렬식(Determinant) 계산을 통한 위상 공간 볼륨 측정
        # 정상 유저: 인간의 행동은 불규칙하므로 4차원 공간의 부피(det)가 큼.
        # 매크로/봇: 고정된 자동화 규칙에 의해 4차원 축들이 서로 선형 종속(Linear Dependent) 관계를 맺음.
        #           이로 인해 차원이 납작하게 짜부라지며 det 가 정확히 0.0으로 수렴함.
        try:
            det = np.linalg.det(covariance_matrix)
        except np.linalg.LinAlgError:
            det = 0.0 # 수치해석적 불안정 시 붕괴로 판정

        # 5. 아노말리 판정 및 차단 시그널 리턴
        if det < self.threshold:
            # 타겟 유저 세션 위상 붕괴 검출 완료 (어뷰징 매크로 확정)
            self._evict_user_history(user_id)
            return False, float(det) # False 리턴 시 Rust 단에서 emergency_gating_lock() 즉시 집행

        return True, float(det)

    def _evict_user_history(self, user_id: int):
        if user_id in self.user_timelines:
            del self.user_timelines[user_id]

