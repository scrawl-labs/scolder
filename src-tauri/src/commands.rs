// commands.rs
// Tauri 커맨드 핸들러 - 프론트엔드(React)와 Rust 백엔드를 연결
//
// 핵심 Rust 개념:
// - Arc<Mutex<T>>: 여러 스레드가 안전하게 공유하는 값
// - AtomicBool: 락 없이 원자적으로 읽고 쓸 수 있는 bool
// - mpsc 채널: 스레드 간 메시지 전달
// - tauri::State: 앱 전역 상태 주입

use crate::{audio, session, transcribe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use tauri::{AppHandle, Emitter, State};

// ─── 앱 전역 상태 ────────────────────────────────────────────────────────────

/// Tauri `manage()`로 등록되는 전역 앱 상태
/// AppState는 여러 커맨드에서 공유되므로 Send + Sync가 필요
pub struct AppState {
    /// 녹음 중 여부 (원자적 bool - 락 없이 안전)
    pub is_recording: Arc<AtomicBool>,

    /// 세션 시작 처리 중 여부 - 권한 프롬프트 응답을 기다리는 동안 true
    /// is_recording과 분리: get_recording_status 폴링이 "대기 중"을 "녹음 중"으로 보지 않게 함
    pub is_starting: Arc<AtomicBool>,

    /// 파일 업로드 처리 중 여부 (원자적 bool - 락 없이 안전)
    /// 라이브 녹음(is_recording)과 파일 업로드가 동시에 진행되지 않도록 상호 배제하는 데 사용
    pub is_processing_upload: Arc<AtomicBool>,

    /// 세션의 모든 발화 기록 (피드백 포함) - STOP & SAVE 이후에만 채워짐
    pub utterances: Arc<Mutex<Vec<session::Utterance>>>,

    /// 녹음 중 실시간으로 누적되는 원문 전사(전문) - LLM 호출 없이 타임스탬프+텍스트만 저장
    pub raw_utterances: Arc<Mutex<Vec<session::RawUtterance>>>,

    /// 세션 시작 시각
    pub session_start: Arc<Mutex<Option<chrono::DateTime<chrono::Local>>>>,

    /// 오디오 캡처 스레드 종료 신호 채널
    /// Sender를 Some으로 설정하면 오디오 스레드가 실행 중
    pub stop_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,

    /// Whisper+피드백 처리 태스크의 핸들
    /// stop_session에서 이 핸들을 await해 처리 큐가 다 빌 때까지 기다림
    /// (그렇지 않으면 아직 전사/피드백 중인 발화가 리포트에서 누락됨)
    pub processing_handle: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,

    /// 백그라운드로 띄운 `ollama serve` 프로세스 - 앱 종료 시 kill해서 고아 프로세스 방지
    pub ollama_process: Arc<Mutex<Option<std::process::Child>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            is_starting: Arc::new(AtomicBool::new(false)),
            is_processing_upload: Arc::new(AtomicBool::new(false)),
            utterances: Arc::new(Mutex::new(Vec::new())),
            raw_utterances: Arc::new(Mutex::new(Vec::new())),
            session_start: Arc::new(Mutex::new(None)),
            stop_tx: Arc::new(Mutex::new(None)),
            processing_handle: Arc::new(Mutex::new(None)),
            ollama_process: Arc::new(Mutex::new(None)),
        }
    }
}

/// `is_processing_upload` 플래그를 함수 종료 시점(성공/에러 조기 반환 모두)에
/// 자동으로 false로 되돌리는 RAII 가드. `start_session_from_file`처럼 이른 반환
/// 지점이 여러 개인 함수에서 플래그 리셋을 매 반환 경로마다 수동으로 적어주지
/// 않아도 되게 해준다 - Rust의 Drop은 스코프를 벗어나는 모든 경로(return 포함)에서
/// 호출이 보장된다.
struct UploadGuard(Arc<AtomicBool>);

impl Drop for UploadGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ─── Tauri 커맨드들 ───────────────────────────────────────────────────────────

/// 첫 실행 설치 전체를 한번에 처리: Whisper 모델 다운로드(필요 시) →
/// Ollama 런타임 설치/서버 기동 → LLM 모델 다운로드(필요 시).
/// 진행 상황은 "setup-progress" 이벤트로 계속 emit되므로, 프론트는 invoke 완료를
/// 기다리는 동안 그 이벤트를 구독해 진행률 UI를 그리면 됨.
/// 이미 다 준비돼 있으면 각 단계를 건너뛰고 빠르게 끝남 - 앱 켤 때마다 안전하게 호출 가능.
#[tauri::command]
pub async fn run_setup(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // 이미 이 프로세스에서 Ollama 서버를 띄워뒀다면 다시 기동할 필요 없음
    if state.ollama_process.lock().unwrap().is_some() {
        return Ok(());
    }

    let child = crate::setup::run(&app).await.map_err(|e| e.to_string())?;
    *state.ollama_process.lock().unwrap() = Some(child);
    Ok(())
}

/// is_starting 플래그를 선점하고, 어떤 경로로 반환되든(에러·성공·패닉) drop 시 해제
struct StartingGuard<'a>(&'a AtomicBool);

impl<'a> StartingGuard<'a> {
    fn claim(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for StartingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 세션 시작 전 확인: 업로드 동시 진행, Whisper 모델, 마이크 권한
/// 권한이 미결정이면 시스템 프롬프트 응답을 타임아웃 없이 기다림 -
/// 이후 캡처 시작 타임아웃에는 사용자 응답 시간이 포함되지 않음
async fn precheck_session_start(state: &AppState) -> Result<(), String> {
    // 파일 업로드 처리 중이면 마이크 세션을 시작하지 않음 (동시 진행 방지)
    if state.is_processing_upload.load(Ordering::SeqCst) {
        return Err("A file is being processed. Please try again when it finishes.".to_string());
    }

    if !transcribe::is_model_ready() {
        return Err(transcribe::model_download_instructions());
    }

    crate::mic_permission::ensure_microphone_access().await?;
    Ok(())
}

/// 영어 수업 세션 시작
///
/// 1. 마이크 오디오 캡처 스레드 시작
/// 2. 오디오 처리(Whisper) 비동기 태스크 시작 - 녹음 중에는 전사만 하고 LLM은 호출하지 않음
///    (실시간 피드백은 MVP에서 제외 - 세션 전체 문맥으로 STOP & SAVE 시점에 일괄 생성)
#[tauri::command]
pub async fn start_session(state: State<'_, AppState>) -> Result<(), String> {
    // 시작 처리를 원자적으로 선점 - 권한 프롬프트 응답을 기다리는 동안(await) 사용자가
    // New Session을 다시 눌러도 두 번째 호출은 여기서 거절됨
    let _starting = StartingGuard::claim(&state.is_starting)
        .ok_or_else(|| "A session is already starting".to_string())?;

    if state.is_recording.load(Ordering::SeqCst) {
        return Err("A session is already in progress".to_string());
    }

    precheck_session_start(&state).await?;

    // 이전 세션 데이터 초기화
    state.utterances.lock().unwrap().clear();
    state.raw_utterances.lock().unwrap().clear();
    *state.session_start.lock().unwrap() = Some(chrono::Local::now());
    state.is_recording.store(true, Ordering::SeqCst);

    // 오디오 채널: 발화 단위 오디오 데이터 전달
    // SyncSender: 버퍼 크기 8 - 꽉 차면 try_send가 에러 반환 (블로킹 없음)
    let (audio_tx, audio_rx) = mpsc::sync_channel::<Vec<f32>>(8);

    // 오디오 스레드 종료 채널
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    *state.stop_tx.lock().unwrap() = Some(stop_tx);

    // 오디오 캡처가 실제로 시작됐는지 확인하는 채널 (마이크 권한 실패 등을 프론트로 전달)
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

    // ── 오디오 캡처 스레드 ────────────────────────────────────────────────────
    // cpal::Stream이 Send가 아닐 수 있으므로 별도 OS 스레드에서 소유
    std::thread::spawn(move || {
        let stream = match audio::start_capture(audio_tx) {
            Ok(s) => {
                let _ = ready_tx.send(Ok(()));
                s
            }
            Err(e) => {
                eprintln!("[Audio Thread] 캡처 시작 실패: {e}");
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
        };

        // stop_rx.recv()가 블로킹 - stop_tx가 drop되거나 메시지 수신 시 종료
        let _ = stop_rx.recv();
        drop(stream); // Stream을 drop하면 cpal 캡처 중단
        println!("[Audio Thread] 종료");
    });

    // 캡처 스레드가 실제로 마이크를 열었는지 최대 5초 대기 후 확인
    // (recv_timeout은 블로킹이므로 spawn_blocking으로 감싸 tokio 런타임을 막지 않음)
    let capture_ready = tauri::async_runtime::spawn_blocking(move || {
        ready_rx.recv_timeout(std::time::Duration::from_secs(5))
    })
    .await;

    match capture_ready {
        Ok(Ok(Ok(()))) => {
            // 캡처 정상 시작
        }
        Ok(Ok(Err(e))) => {
            state.is_recording.store(false, Ordering::SeqCst);
            *state.stop_tx.lock().unwrap() = None;
            return Err(format!(
                "Could not start microphone capture: {e}\n\nCheck that Scoldler is enabled in System Settings → Privacy & Security → Microphone."
            ));
        }
        Ok(Err(_)) | Err(_) => {
            state.is_recording.store(false, Ordering::SeqCst);
            *state.stop_tx.lock().unwrap() = None;
            return Err(
                "Timed out starting microphone capture. Check your microphone and its permission."
                    .to_string(),
            );
        }
    }

    // ── 처리 태스크 (async) ───────────────────────────────────────────────────
    // tauri::async_runtime::spawn: Tauri의 tokio 런타임에서 비동기 실행
    // 녹음 중에는 Whisper 전사만 수행 - LLM 호출 없이 원문만 누적
    let raw_utterances = Arc::clone(&state.raw_utterances);

    let handle = tauri::async_runtime::spawn(async move {
        println!("[Processing] 오디오 처리 시작");

        loop {
            // try_recv: 블로킹 없이 데이터 확인
            // 종료 조건은 오직 Disconnected뿐 - is_recording=false가 되어도
            // 채널에 남은 오디오(아직 전사 안 된 마지막 발화들)를 끝까지 처리함
            match audio_rx.try_recv() {
                Ok(audio_data) => {
                    // Whisper STT: CPU 집약적이므로 블로킹 스레드에서 실행
                    let text =
                        match tauri::async_runtime::spawn_blocking(move || {
                            transcribe::transcribe(&audio_data)
                        })
                        .await
                        {
                            Ok(Ok(t)) if !t.is_empty() => t,
                            Ok(Ok(_)) => {
                                println!("[Transcribe] 빈 텍스트 - 건너뜀");
                                continue;
                            }
                            Ok(Err(e)) => {
                                eprintln!("[Transcribe] 실패: {e}");
                                continue;
                            }
                            Err(e) => {
                                eprintln!("[Transcribe] 태스크 에러: {e}");
                                continue;
                            }
                        };

                    println!("[Transcribe] 텍스트: \"{text}\"");

                    let timestamp =
                        chrono::Local::now().format("%H:%M:%S").to_string();

                    // 전문(raw transcript)에 추가 - 피드백은 STOP & SAVE 시점에 일괄 생성
                    raw_utterances.lock().unwrap().push(session::RawUtterance {
                        timestamp,
                        text,
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // 데이터 없음 - 100ms 대기
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    println!("[Processing] 오디오 채널 끊김 - 종료");
                    break;
                }
            }
        }

        println!("[Processing] 오디오 처리 종료");
    });

    *state.processing_handle.lock().unwrap() = Some(handle);

    Ok(())
}

/// 세션 종료 - 오디오 중단 + 전문 기반 리포트 일괄 생성 + 저장
///
/// 녹음 중에는 LLM을 호출하지 않았으므로, 여기서 누적된 전문(raw_utterances)을
/// 하나씩 돌며 피드백을 생성한다. 발화 수 × LLM 왕복 시간만큼 걸릴 수 있어
/// 프론트는 이 커맨드의 await가 끝날 때까지를 "리포트 생성 중"으로 표시하면 된다.
///
/// # 반환값
/// 저장된 마크다운 파일 경로
#[tauri::command]
pub async fn stop_session(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    state.is_recording.store(false, Ordering::SeqCst);

    // 오디오 스레드에 종료 신호 전송 (stop_tx를 drop하면 recv가 해제됨)
    // → 캡처 스레드가 stream을 drop → audio_tx도 함께 drop → audio_rx가 결국 Disconnected
    *state.stop_tx.lock().unwrap() = None;

    // 처리 태스크가 남은 오디오(아직 전사 대기 중인 마지막 발화들)를 모두 소진할 때까지 대기.
    // 최대 30초 타임아웃 - Whisper가 멈춰도 앱이 무한 대기하지 않도록 방어
    let handle = state.processing_handle.lock().unwrap().take();
    if let Some(handle) = handle {
        if tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .is_err()
        {
            eprintln!("[Session] 처리 태스크 대기 시간 초과 - 지금까지의 전문만 저장");
        }
    }

    let raw_utterances = state.raw_utterances.lock().unwrap().clone();
    let session_start = state.session_start.lock().unwrap().clone();

    let Some(start) = session_start else {
        return Ok(String::new());
    };

    let path = session::finalize_session(
        &app,
        &raw_utterances,
        &start,
        session::SessionSource::Recorded,
    )
    .await
    .map_err(|e| e.to_string())?;

    *state.utterances.lock().unwrap() = session::read_utterances(&path).unwrap_or_default();

    Ok(path)
}

/// 업로드된 오디오 파일로부터 세션 생성 - 마이크 캡처 없이 파일 하나를
/// 디코딩 → 전사(세그먼트 타임스탬프) → 피드백 생성까지 한 번에 처리한다.
/// 라이브 세션과 달리 시작/종료 단계가 없고, 이 커맨드 하나가 끝나면
/// 바로 완성된 리포트 경로를 반환한다.
#[tauri::command]
pub async fn start_session_from_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    if state.is_recording.load(Ordering::SeqCst) || state.is_starting.load(Ordering::SeqCst) {
        return Err("A session is already in progress".to_string());
    }

    // 이미 다른 파일 업로드를 처리 중이면 에러 (동시 업로드 방지)
    if state.is_processing_upload.load(Ordering::SeqCst) {
        return Err("A file is already being processed".to_string());
    }

    if !transcribe::is_model_ready() {
        return Err(transcribe::model_download_instructions());
    }

    // 여기부터는 모든 사전 검증(guard)을 통과했으므로 처리 중 플래그를 세운다.
    // _upload_guard는 이 함수가 어떤 경로로 반환하든(성공/에러 조기 반환 모두) Drop 시점에
    // 플래그를 자동으로 false로 되돌려, 실패한 업로드가 이후 세션을 영구히 막지 않도록 한다.
    state.is_processing_upload.store(true, Ordering::SeqCst);
    let _upload_guard = UploadGuard(Arc::clone(&state.is_processing_upload));

    let file_path = std::path::PathBuf::from(&path);
    let original_filename = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    let app_for_decode = app.clone();
    let pcm = tauri::async_runtime::spawn_blocking(move || {
        let _ = app_for_decode.emit(
            "session-progress",
            serde_json::json!({ "stage": "decoding_audio", "current": 0, "total": 0 }),
        );
        crate::decode::decode_file_to_pcm16k(&file_path)
    })
    .await
    .map_err(|e| format!("Decoding task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    let app_for_transcribe = app.clone();
    let segments = tauri::async_runtime::spawn_blocking(move || {
        crate::transcribe::transcribe_with_segments(&pcm, move |percent| {
            let _ = app_for_transcribe.emit(
                "session-progress",
                serde_json::json!({ "stage": "transcribing", "current": percent, "total": 100 }),
            );
        })
    })
    .await
    .map_err(|e| format!("Transcription task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    if segments.is_empty() {
        return Err("No speech was detected".to_string());
    }

    let raw_utterances: Vec<session::RawUtterance> = segments
        .into_iter()
        .map(|seg| session::RawUtterance {
            timestamp: format_offset_timestamp(seg.start_secs),
            text: seg.text,
        })
        .collect();

    *state.raw_utterances.lock().unwrap() = raw_utterances.clone();

    let start = chrono::Local::now();
    let report_path = session::finalize_session(
        &app,
        &raw_utterances,
        &start,
        session::SessionSource::Uploaded { original_filename },
    )
    .await
    .map_err(|e| e.to_string())?;

    *state.utterances.lock().unwrap() =
        session::read_utterances(&report_path).unwrap_or_default();

    Ok(report_path)
}

/// 파일 시작 기준 경과 시간(초)을 "mm:ss" 형식으로 포맷
/// 라이브 세션의 타임스탬프(HH:MM:SS 벽시계 시각)와 의미가 다름 - 업로드 세션은
/// "몇 분 몇 초 지점의 발화인가"가 사용자에게 더 유용함
fn format_offset_timestamp(seconds: f32) -> String {
    let total_secs = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// 현재 세션의 모든 발화 기록 반환 (STOP & SAVE 이후에만 값이 채워짐)
#[tauri::command]
pub fn get_utterances(state: State<'_, AppState>) -> Vec<session::Utterance> {
    state.utterances.lock().unwrap().clone()
}

/// 녹음 중 실시간으로 누적되는 원문 전사(전문) 반환 - "전문 보기" 버튼용
#[tauri::command]
pub fn get_transcript(state: State<'_, AppState>) -> Vec<session::RawUtterance> {
    state.raw_utterances.lock().unwrap().clone()
}

/// 현재 녹음 상태 반환
#[tauri::command]
pub fn get_recording_status(state: State<'_, AppState>) -> bool {
    state.is_recording.load(Ordering::SeqCst)
}

/// 저장된 모든 세션 목록 조회
#[tauri::command]
pub fn list_sessions() -> Result<Vec<session::SessionInfo>, String> {
    session::list_sessions().map_err(|e| e.to_string())
}

/// 전체 세션 합산 통계 (idle 화면 대시보드용)
#[tauri::command]
pub fn get_stats() -> Result<session::Stats, String> {
    session::get_stats().map_err(|e| e.to_string())
}

/// 주어진 .md 경로로 SessionInfo를 직접 구성 (STOP & SAVE 직후 방금 만든 세션을
/// "선택된 세션"으로 바로 지정하기 위해 사용 - list_sessions 순회 없이 즉시 반환)
#[tauri::command]
pub fn get_session_info(path: String) -> session::SessionInfo {
    session::get_session_info(&path)
}

/// 특정 세션 마크다운 내용 읽기
#[tauri::command]
pub fn read_session(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read session: {}", e))
}

/// 특정 세션의 구조화된 발화 기록 읽기 (JSON 사이드카)
/// 라이브 리포트와 동일한 카드/표 UI로 렌더링하기 위해 사용.
/// JSON이 없는 옛날 세션이면 빈 배열 반환 → 프론트에서 마크다운으로 폴백
#[tauri::command]
pub fn read_session_utterances(path: String) -> Result<Vec<session::Utterance>, String> {
    session::read_utterances(&path).map_err(|e| e.to_string())
}

/// 이 세션에 구조화된 발화 기록(JSON 사이드카)이 있는지 여부
/// 프론트에서 "발화 0건인 최신 세션"과 "JSON이 없는 옛날 세션"을 구분해
/// 마크다운 폴백 여부를 결정하는 데 사용
#[tauri::command]
pub fn session_has_utterances_json(path: String) -> bool {
    session::has_utterances_json(&path)
}

/// 특정 세션의 전문(.txt) 읽기 - "전문 보기" 버튼용
/// .txt가 없는 세션(과거 세션)이면 빈 문자열 반환
#[tauri::command]
pub fn read_session_transcript(path: String) -> Result<String, String> {
    session::read_transcript(&path).map_err(|e| e.to_string())
}

/// 세션 이름(제목) 변경
#[tauri::command]
pub fn rename_session(path: String, new_title: String) -> Result<(), String> {
    session::rename_session(&path, &new_title).map_err(|e| e.to_string())
}

/// 세션 삭제
#[tauri::command]
pub fn delete_session(path: String) -> Result<(), String> {
    session::delete_session(&path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod starting_guard_tests {
    use super::*;

    #[test]
    fn second_claim_fails_while_first_is_held() {
        let flag = AtomicBool::new(false);
        let first = StartingGuard::claim(&flag);
        assert!(first.is_some());
        assert!(StartingGuard::claim(&flag).is_none());
    }

    #[test]
    fn flag_is_released_on_drop() {
        let flag = AtomicBool::new(false);
        {
            let _g = StartingGuard::claim(&flag).unwrap();
            assert!(flag.load(Ordering::SeqCst));
        }
        assert!(!flag.load(Ordering::SeqCst));
        assert!(StartingGuard::claim(&flag).is_some());
    }
}
