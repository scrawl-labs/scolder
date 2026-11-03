// session.rs
// 세션 관리 및 마크다운 리포트 저장
//
// 핵심 Rust 개념:
// - struct와 impl: 타입 정의와 메서드
// - String formatting: format! 매크로
// - std::fs: 파일시스템 작업

use crate::{feedback::{self, FeedbackResult}, turns};
use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

/// 한 번의 발화와 그에 대한 피드백
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Utterance {
    /// HH:MM:SS 형식 타임스탬프
    pub timestamp: String,
    pub feedback: FeedbackResult,
}

/// 녹음 중 실시간 LLM 호출 없이 누적되는 원문 발화 (타임스탬프 + 전사 텍스트만)
/// STOP & SAVE 시점에 이 목록 전체를 기반으로 피드백을 일괄 생성함
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawUtterance {
    pub timestamp: String,
    pub text: String,
}

/// 세션이 라이브 녹음으로 만들어졌는지, 파일 업로드로 만들어졌는지
/// `<base>.source` 사이드카 파일로 저장됨 (.title/.txt와 동일한 패턴)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionSource {
    Recorded,
    Uploaded { original_filename: String },
}

/// `<base>.source` 사이드카에 세션 출처를 저장
fn save_source_sidecar(md_path: &PathBuf, source: &SessionSource) -> Result<()> {
    let path = md_path.with_extension("source");
    let content = match source {
        SessionSource::Recorded => "recorded".to_string(),
        SessionSource::Uploaded { original_filename } => {
            format!("uploaded\n{original_filename}")
        }
    };
    std::fs::write(path, content)?;
    Ok(())
}

/// `<base>.source` 사이드카를 읽음 - 없으면(과거 세션) Recorded로 취급
fn read_source_sidecar(md_path: &PathBuf) -> SessionSource {
    let path = md_path.with_extension("source");
    let Ok(content) = std::fs::read_to_string(path) else {
        return SessionSource::Recorded;
    };
    let mut lines = content.lines();
    match lines.next() {
        Some("uploaded") => SessionSource::Uploaded {
            original_filename: lines.next().unwrap_or("").to_string(),
        },
        _ => SessionSource::Recorded,
    }
}

/// 리포트 생성(발화별 LLM 피드백) 진행률 - "session-progress" 이벤트로 emit됨
#[derive(Debug, Clone, Serialize)]
struct SessionProgress {
    stage: String, // "generating_feedback"
    current: usize,
    total: usize,
}

/// 전문(raw_utterances)을 바탕으로 발화별 LLM 피드백을 일괄 생성하고 리포트를
/// 저장한다. 라이브 녹음(stop_session)과 파일 업로드(start_session_from_file)가
/// 이 함수 하나를 공유해서 리포트 생성 로직이 완전히 하나로 유지된다.
///
/// # 반환값
/// 저장된 마크다운 파일의 절대 경로 (문자열)
pub async fn finalize_session(
    app: &AppHandle,
    raw_utterances: &[RawUtterance],
    start_time: &DateTime<Local>,
    source: SessionSource,
) -> Result<String> {
    // 전문(.txt)은 피드백 생성 여부와 무관하게 먼저 저장 - 실패해도 원문은 남도록
    if let Err(e) = save_transcript(raw_utterances, start_time) {
        eprintln!("[Session] 전문 저장 실패: {e}");
    }

    // 짧게 끊긴 전사 조각들을 말하기 턴으로 묶고 잡음/추임새는 제외 (turns.rs 참고)
    let turns = turns::group_into_turns(raw_utterances);

    let total = turns.len();
    let mut utterances = Vec::with_capacity(total);
    for (i, turn) in turns.iter().enumerate() {
        let _ = app.emit(
            "session-progress",
            SessionProgress {
                stage: "generating_feedback".to_string(),
                current: i + 1,
                total,
            },
        );

        // 앞 2턴 + 뒤 1턴을 문맥으로 함께 보냄 - 조각만 보고 의미를 지어내지 않도록
        let before: Vec<&str> = turns[i.saturating_sub(2)..i]
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        let after: Vec<&str> = turns
            .get(i + 1)
            .map(|t| vec![t.text.as_str()])
            .unwrap_or_default();

        let fb = match feedback::get_feedback(&turn.text, &before, &after).await {
            Ok(fb) => fb,
            Err(e) => {
                eprintln!("[Feedback] \"{}\" 처리 실패: {e}", turn.text);
                feedback::empty_feedback(&turn.text)
            }
        };
        utterances.push(Utterance {
            timestamp: turn.timestamp.clone(),
            feedback: fb,
        });
    }

    let path = save_report(&utterances, start_time)?;
    if let Err(e) = save_source_sidecar(&path, &source) {
        eprintln!("[Session] 출처 사이드카 저장 실패: {e}");
    }

    Ok(path.to_string_lossy().to_string())
}

/// 세션 리포트 저장 디렉터리
/// ~/.wtf-english/sessions/
pub fn get_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wtf-english")
        .join("sessions")
}

/// 세션 리포트를 마크다운 파일로 저장
///
/// # 반환값
/// 저장된 파일의 절대 경로
pub fn save_report(utterances: &[Utterance], start_time: &DateTime<Local>) -> Result<PathBuf> {
    let dir = get_sessions_dir();
    // create_dir_all: 중간 디렉터리도 모두 생성 (mkdir -p)
    std::fs::create_dir_all(&dir)?;

    let base_name = start_time.format("%Y-%m-%d_%H-%M-%S").to_string();
    let path = dir.join(format!("{base_name}.md"));

    let content = build_markdown(utterances, start_time);
    std::fs::write(&path, content)?;

    // 마크다운과 별개로 발화 원본 데이터를 JSON으로도 저장
    // → 나중에 이 세션을 다시 열 때, 라이브 리포트와 동일한 UI(카드/표)로 렌더링 가능
    let json_path = dir.join(format!("{base_name}.json"));
    std::fs::write(&json_path, serde_json::to_string_pretty(utterances)?)?;

    println!("[Session] 리포트 저장: {:?}", path);
    Ok(path)
}

/// 이 세션에 구조화된 발화 기록(JSON 사이드카)이 있는지 여부
/// JSON이 없는 옛날 세션과, JSON은 있지만 발화가 0건인 최신 세션을 구분하기 위해 사용
/// (둘 다 read_utterances가 빈 벡터를 반환하므로 벡터 길이만으로는 구분 불가)
pub fn has_utterances_json(md_path: &str) -> bool {
    PathBuf::from(md_path).with_extension("json").exists()
}

/// 마크다운 파일 경로로부터 같은 세션의 JSON(구조화된 발화 기록)을 읽음
/// 이 JSON이 없으면(이전 버전에서 저장된 세션) 빈 벡터 반환
pub fn read_utterances(md_path: &str) -> Result<Vec<Utterance>> {
    let json_path = PathBuf::from(md_path).with_extension("json");
    if !json_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(json_path)?;
    Ok(serde_json::from_str(&content)?)
}

/// 원문 전사 전체(전문)를 .txt 사이드카로 저장
/// base_name은 save_report와 동일한 규칙(start_time 기준)으로 계산해 파일명을 맞춤
pub fn save_transcript(raw: &[RawUtterance], start_time: &DateTime<Local>) -> Result<PathBuf> {
    let dir = get_sessions_dir();
    std::fs::create_dir_all(&dir)?;

    let base_name = start_time.format("%Y-%m-%d_%H-%M-%S").to_string();
    let path = dir.join(format!("{base_name}.txt"));

    let mut content = String::new();
    for u in raw {
        content.push_str(&format!("[{}] {}\n", u.timestamp, u.text));
    }
    std::fs::write(&path, content)?;

    Ok(path)
}

/// 마크다운 파일 경로로부터 같은 세션의 전문(.txt)을 읽음
/// 없으면(과거 세션 또는 발화가 없던 세션) 빈 문자열 반환
pub fn read_transcript(md_path: &str) -> Result<String> {
    let txt_path = PathBuf::from(md_path).with_extension("txt");
    if !txt_path.exists() {
        return Ok(String::new());
    }
    Ok(std::fs::read_to_string(txt_path)?)
}

/// 마크다운 리포트 생성
fn build_markdown(utterances: &[Utterance], start_time: &DateTime<Local>) -> String {
    let total = utterances.len();
    let error_count = utterances.iter().filter(|u| u.feedback.has_error).count();
    let vocab_items: Vec<_> = utterances
        .iter()
        .filter_map(|u| u.feedback.idiom_or_vocab.as_ref())
        .collect();

    // 여러 줄의 문자열을 조합할 때 String::new() + push_str 패턴
    let mut md = String::new();

    md.push_str(&format!(
        "# 🎙️ English Session — {}\n\n",
        start_time.format("%Y년 %m월 %d일 %H:%M")
    ));

    // 요약 섹션
    md.push_str("## 📊 요약\n\n");
    md.push_str(&format!("- 총 발화: **{total}**문장\n"));
    md.push_str(&format!("- 교정 필요: **{error_count}**문장\n"));
    md.push_str(&format!(
        "- 새로운 표현/어휘: **{}**개\n\n",
        vocab_items.len()
    ));

    // 교정 목록
    let errors: Vec<_> = utterances.iter().filter(|u| u.feedback.has_error).collect();
    if !errors.is_empty() {
        md.push_str("## 🔴 교정 목록\n\n");
        md.push_str("| 내가 한 말 | 교정 | 이유 | 더 자연스럽게 |\n");
        md.push_str("|:----------|:-----|:-----|:--------------|\n");
        for u in &errors {
            let corrected = u.feedback.corrected.as_deref().unwrap_or("—");
            let why = u.feedback.explanation.as_deref().unwrap_or("—");
            let better = u.feedback.better_expression.as_deref().unwrap_or("—");
            md.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                u.feedback.original, corrected, why, better
            ));
        }
        md.push('\n');
    }

    // 어휘/관용어 섹션
    if !vocab_items.is_empty() {
        md.push_str("## 📚 오늘 배운 표현\n\n");
        for item in vocab_items {
            md.push_str(&format!("- **{}**\n", item));
        }
        md.push('\n');
    }

    // 전체 발화 기록
    if !utterances.is_empty() {
        md.push_str("## 📝 전체 발화 기록\n\n");
        for u in utterances {
            md.push_str(&format!("**[{}]** {}\n", u.timestamp, u.feedback.original));
            if let Some(ref c) = u.feedback.corrected {
                md.push_str(&format!("→ 교정: _{c}_\n"));
            }
            if let Some(ref why) = u.feedback.explanation {
                md.push_str(&format!("→ 이유: {why}\n"));
            }
            if let Some(ref b) = u.feedback.better_expression {
                md.push_str(&format!("→ 더 자연스럽게: _{b}_\n"));
            }
            md.push('\n');
        }
    }

    md
}

/// 모든 세션을 합산한 전체 통계 - idle 화면 대시보드용
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Stats {
    pub total_sessions: usize,
    pub total_utterances: usize,
    pub total_corrections: usize,
}

/// 저장된 모든 세션의 JSON 사이드카를 순회해 통계를 합산
/// 세션 수가 많지 않은 개인용 앱 특성상 매번 전체를 읽어도 충분히 빠름
pub fn get_stats() -> Result<Stats> {
    let sessions = list_sessions()?;
    let mut total_utterances = 0;
    let mut total_corrections = 0;

    for s in &sessions {
        let utterances = read_utterances(&s.path)?;
        total_utterances += utterances.len();
        total_corrections += utterances.iter().filter(|u| u.feedback.has_error).count();
    }

    Ok(Stats {
        total_sessions: sessions.len(),
        total_utterances,
        total_corrections,
    })
}

/// 저장된 모든 세션 목록 반환
/// 최신순으로 정렬
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionInfo {
    pub filename: String,  // "2025-01-20_14-30-45.md"
    pub title: String,     // "Jan 20, 2:30 PM"
    pub path: String,      // 절대 경로
    pub source: SessionSource,
}

pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    let dir = get_sessions_dir();
    
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().map_or(false, |ext| ext == "md") {
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                // 사용자가 직접 지정한 이름(.title 사이드카)이 있으면 그것을, 없으면 타임스탬프에서 포맷팅한 제목을 사용
                let title = read_custom_title(&path)
                    .unwrap_or_else(|| format_session_title(filename));

                sessions.push(SessionInfo {
                    filename: filename.to_string(),
                    title,
                    path: path.to_string_lossy().to_string(),
                    source: read_source_sidecar(&path),
                });
            }
        }
    }

    // 최신순 정렬 (역순)
    sessions.sort_by(|a, b| b.filename.cmp(&a.filename));

    Ok(sessions)
}

/// 주어진 .md 경로 하나로 SessionInfo를 직접 구성
/// STOP & SAVE 직후 방금 만든 세션을 목록에서 "찾는" 대신 바로 만들어 쓰기 위함
/// (list_sessions 전체를 순회해 경로 문자열을 비교하는 것보다 더 직접적이고 안전함)
pub fn get_session_info(md_path: &str) -> SessionInfo {
    let path = PathBuf::from(md_path);
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let title =
        read_custom_title(&path).unwrap_or_else(|| format_session_title(&filename));

    SessionInfo {
        filename,
        title,
        path: md_path.to_string(),
        source: read_source_sidecar(&path),
    }
}

/// <base>.title 사이드카 파일에서 사용자 지정 제목을 읽음 (없으면 None)
fn read_custom_title(md_path: &PathBuf) -> Option<String> {
    let title_path = md_path.with_extension("title");
    std::fs::read_to_string(title_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 세션 이름 변경 - <base>.title 파일에 새 제목 저장
pub fn rename_session(md_path: &str, new_title: &str) -> Result<()> {
    let title_path = PathBuf::from(md_path).with_extension("title");
    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        // 빈 제목이면 사이드카를 지우고 기본 타임스탬프 제목으로 되돌림
        let _ = std::fs::remove_file(title_path);
        return Ok(());
    }
    std::fs::write(title_path, trimmed)?;
    Ok(())
}

/// 세션 삭제 - .md, .json, .title, .txt, .source 사이드카를 모두 삭제 (없는 파일은 무시)
pub fn delete_session(md_path: &str) -> Result<()> {
    let path = PathBuf::from(md_path);
    let _ = std::fs::remove_file(path.with_extension("json"));
    let _ = std::fs::remove_file(path.with_extension("title"));
    let _ = std::fs::remove_file(path.with_extension("txt"));
    let _ = std::fs::remove_file(path.with_extension("source"));
    std::fs::remove_file(&path)?;
    Ok(())
}

fn format_session_title(filename: &str) -> String {
    // "2025-01-20_14-30-45.md" → "Jan 20, 2:30 PM"
    filename
        .strip_suffix(".md")
        .and_then(|s| {
            let parts: Vec<&str> = s.split('_').collect();
            if parts.len() == 2 {
                let date = parts[0];
                let time = parts[1];
                
                let date_parts: Vec<&str> = date.split('-').collect();
                let time_parts: Vec<&str> = time.split('-').collect();
                
                if date_parts.len() == 3 && time_parts.len() == 3 {
                    let month = date_parts[1];
                    let day = date_parts[2];
                    let hour = time_parts[0];
                    let min = time_parts[1];
                    
                    let months = [
                        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun",
                        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
                    ];
                    
                    if let (Ok(m), Ok(d), Ok(h), Ok(min_num)) =
                        (month.parse::<usize>(), day.parse::<u32>(), hour.parse::<u32>(), min.parse::<u32>())
                    {
                        if m > 0 && m <= 12 {
                            let am_pm = if h < 12 { "AM" } else { "PM" };
                            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
                            return Some(format!(
                                "{} {}, {}:{:02} {}",
                                months[m], d, h12, min_num, am_pm
                            ));
                        }
                    }
                }
            }
            None
        })
        .unwrap_or_else(|| filename.to_string())
}
