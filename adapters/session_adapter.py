# adapters/session_adapter.py
import mmh3  # MurmurHash3를 사용해 문자열을 초고속 고정 정수 인덱스로 변환
from typing import Dict, Tuple

class SessionAdapter:
    def __init__(self, max_profile_slots: int = 1000000):
        """
        :param max_profile_slots: 하드 락킹된 고정 버퍼 내 프로필 주소 변환 최대 슬롯 수
        """
        self.max_profile_slots = max_profile_slots
        
        # 가변 길이 프로필 URL 문자열과 고정 인덱스 간의 역참조 테이블
        # (프로덕션 환경에서는 이 메모리 역시 정적 공유 메모리 공간에 락킹할 수 있습니다)
        self.profile_string_pool: Dict[int, str] = {}

    def ingest_user_request(self, raw_http_request: dict) -> Tuple[int, int, int]:
        """
        인입된 가변 HTTP/WebSocket 요청을 분석하여 
        Sovereign Buffer Cache가 즉시 소화할 수 있는 정수 포인터 축으로 변환(Adapt)합니다.
        
        :param raw_http_request: 가변 데이터를 포함한 원본 HTTP 요청 오브젝트 예시
        :return: (user_id, session_expiry_tick, profile_hash_idx)
        """
        # 1. 가변 문자열 유저 고유 토큰/식별자를 u64 정수로 파싱
        # 문자열 헤더나 쿠키를 파싱하되, 유저 공간의 힙 메모리 생성을 최소화합니다.
        raw_user_id = raw_http_request.get("user_id", "0")
        user_id = int(raw_user_id) if raw_user_id.isdigit() else 0
        
        # 2. 세션 만료 시간을 커널 틱(또는 에포크 밀리초) 정수로 다이렉트 매핑
        expiry_tick = int(raw_http_request.get("expiry_timestamp", 0))

        # 3. 🔥 [핵심 기믹] 가변 길이 프로필 URL의 고정 정수 ID화
        # 레거시 방식: "https://server.com" (가변 크기 할당 유발)
        # 어댑터 방식: 문자열을 무복사(Zero-copy) 해싱하여 정적 배열의 고정 오프셋 인덱스로 치환
        profile_url = raw_http_request.get("profile_url", "")
        
        if not profile_url:
            profile_hash_idx = 0
        else:
            # MurmurHash3를 이용하여 문자열을 시드값 기반 32비트 무부호 정수로 초고속 변환
            raw_hash = mmh3.hash(profile_url, seed=42, signed=False)
            profile_hash_idx = (raw_hash % (self.max_profile_slots - 1)) + 1
            
            # 주소선 복원을 위해 역참조 풀에 1회 등록 (수정 요청 시 참조용)
            if profile_hash_idx not in self.profile_string_pool:
                self.profile_string_pool[profile_hash_idx] = profile_url

        return user_id, expiry_tick, profile_hash_idx

    def resolve_profile_url(self, profile_hash_idx: int) -> str:
        """
        최하단 커널이나 Rust 프록시가 초고속 라우팅 후 리턴한 profile_hash_idx를 
        다시 유저가 읽을 수 있는 가변 URL 문자열로 복원(Resolve)합니다.
        읽기 핫 패스(Hot-path) 외부에서 클라이언트 응답 직전에 단 1회 수행됩니다.
        """
        return self.profile_string_pool.get(profile_hash_idx, "https://server.com")

