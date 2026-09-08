// -*- coding: utf-8 -*-
// target_proxy_rust/build.rs

use std::env;
use std::path::PathBuf;
use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=../config/system_bounds.toml");
    println!("cargo:rerun-if-changed=../target_hardware_cuda/session_viscosity.cu");

    // 1. 전역 설계 헌법 (system_bounds.toml) 파싱 레이일 가동
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let toml_path = manifest_dir.parent().unwrap().join("config/system_bounds.toml");
    
    let toml_content = fs::read_to_string(&toml_path)
        .expect("[-] 치명적 오류: system_bounds.toml 설정을 로드할 수 없습니다.");
    
    // max_user_slots 정수 상수 기습 추출
    let max_users = toml_content
        .lines()
        .find(|line| line.trim().starts_with("max_user_slots"))
        .and_then(|line| line.split('=').nth(1))
        .map(|val| val.trim().to_string())
        .unwrap_or_else(|| "10000000".to_string());

    // 2. 🎛️ [하드웨어 최적화: nvcc 융합 컴파일러 파이프라인 전개]
    // cc 크레이트 빌드 빌더를 획득하여 엔비디아 네이티브 레지스터 융합 최적화를 강제 집행합니다.
    let mut cuda_build = cc::Build::new();
    
    // 컴파일러를 gcc/clang이 아닌 nvidia nvcc로 하이재킹
    cuda_build.compiler("nvcc");
    
    // 최고 등급 가속 옵션 주입 (-O3, 호스트 네이티브 하드웨어 아키텍처 최적화 직결)
    cuda_build.arg("-O3");
    cuda_build.arg("-Xcompiler");
    cuda_build.arg("-march=native");
    cuda_build.arg("-Xcompiler");
    cuda_build.arg("-fPIC");
    
    // [★ 하드웨어 정렬 싱크]: 구조체 바이트 오프셋 일치를 위한 전역 매크로 상수 인젝션
    cuda_build.arg(format!("-DMAX_USER_SLOTS={}", max_users));

    // 타겟 물리 소스 조크 직결 바인딩
    let cuda_src = manifest_dir.parent().unwrap().join("target_hardware_cuda/session_viscosity.cu");
    cuda_build.file(cuda_src);

    // 3. 0ns 정적 메모리 링킹 집행
    // Rust 컴파일러가 최종 릴리스 바이너리를 조각할 때 
    // 내부 정적 세그먼트에 이 정류된 하드웨어 오브젝트를 무복사(Zero-copy) 융합하도록 명령합니다.
    cuda_build.compile("session_viscosity_hardware_core");

    println!("cargo:rustc-link-lib=static=session_viscosity_hardware_core");
    println!("cargo:rustc-link-lib=dylib=cuda");
    println!("cargo:rustc-link-lib=dylib=cudart");
}
