// feedback.rs
// Ollama LLM을 통한 영어 피드백 생성
//
// 핵심 Rust 개념:
// - async/await: 비동기 프로그래밍
// - serde: 직렬화/역직렬화 (Serialize, Deserialize)
// - reqwest: HTTP 클라이언트

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const OLLAMA_MODEL: &str = "llama3.1:8b";

/// LLM이 반환하는 피드백 구조체
///
/// #[derive(Serialize, Deserialize)]: JSON 변환 자동 구현
/// #[derive(Clone)]: .clone()으로 복사 가능하게
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FeedbackResult {
    /// 내가 한 말 (원문)
    /// LLM 응답 JSON에는 이 필드가 없고, 파싱 후 코드에서 직접 채워 넣음.
    /// serde(default)가 없으면 필드 누락 시 deserialize 자체가 실패함.
    #[serde(default)]
    pub original: String,

    /// 문법 교정 (오류 없으면 None)
    pub corrected: Option<String>,

    /// 더 자연스러운 표현 (이미 자연스러우면 None)
    pub better_expression: Option<String>,

    /// 이 문맥에서 배울 만한 관용어/어휘
    pub idiom_or_vocab: Option<String>,

    /// 무엇이 왜 틀렸는지 한 줄 설명 (예: "came → come: 'hadn't' 뒤에는 과거분사")
    /// 이전 버전 세션 JSON에는 없으므로 serde(default)
    #[serde(default)]
    pub explanation: Option<String>,

    /// 교정이 필요한 문장인지
    pub has_error: bool,
}

/// LLM에게 보내는 프롬프트
///
/// 입력은 실시간 회화를 음성 인식으로 받아 적은 것이라 문장이 중간에 끊기거나
/// 말끝을 흐리는 게 정상이다. 예전 프롬프트는 이걸 전부 "오류"로 보고 억지로
/// 완성하거나(의미가 바뀜) 대소문자/마침표만 바꾼 "교정"을 내놨다.
/// 그래서 (1) 회화체 조각은 오류가 아니라고 못박고 (2) 실제 문법 오류만,
/// 최소한으로, 의미를 유지하며 고치게 하고 (3) 이유를 한 줄로 쓰게 한다.
const SYSTEM_PROMPT: &str = r#"You are an English tutor reviewing a Korean learner's speech from a live video call.
The text is an automatic speech-recognition transcript of casual spoken conversation.

What is NOT an error (never correct these):
- Incomplete sentences, trailing off ("so...", "but-"), restarts, fillers ("like", "you know").
- Missing capitalization or punctuation.
- Casual but correct spoken English ("'cause", "gonna", "Yeah, it's so busy these days").
- Words that look like speech-recognition mistakes (nonsense or out-of-place words). Ignore them.

What IS an error (only these):
- Wrong verb form or tense, subject-verb agreement, articles, singular/plural, prepositions,
  wrong word choice, wrong word order, or an un-English phrase a teacher would clearly fix.

How to correct:
- Change as few words as possible. Keep the speaker's meaning, tense intent and tone.
- Do NOT finish an incomplete sentence or guess what the speaker meant to say next.
- Do NOT invent content that was not said.
- "explanation": one short line naming the exact fix, like "came -> come: after 'hadn't' use the past participle".

"better_expression": only when a native speaker would clearly say it differently. Otherwise null.
"idiom_or_vocab": only if genuinely useful for an intermediate learner, format "phrase: meaning". Otherwise null.

Most casual utterances are fine. When in doubt, return has_error false with nulls.

Return ONLY a JSON object with exactly these fields:
{"has_error": boolean, "corrected": string|null, "explanation": string|null, "better_expression": string|null, "idiom_or_vocab": string|null}

Examples:
Student: "It's almost winter but it- It hadn't came, so... Maybe in December it would be."
{"has_error":true,"corrected":"It's almost winter but it- It hasn't come yet, so... Maybe in December it will.","explanation":"hadn't came -> hasn't come: use present perfect with the past participle 'come'","better_expression":"It's almost winter, but it hasn't come yet. Maybe in December.","idiom_or_vocab":null}

Student: "about one week. Before Christmas. and after Christmas."
{"has_error":false,"corrected":null,"explanation":null,"better_expression":null,"idiom_or_vocab":null}

Student: "My sister don't like it. She said it is too much expensive, so..."
{"has_error":true,"corrected":"My sister doesn't like it. She said it is too expensive, so...","explanation":"don't -> doesn't (she); 'too much expensive' -> 'too expensive'","better_expression":null,"idiom_or_vocab":"pricey: expensive (casual)"}

Student: "Yeah, it's so busy these days."
{"has_error":false,"corrected":null,"explanation":null,"better_expression":null,"idiom_or_vocab":null}"#;

/// 피드백 없이 원문만 담은 결과
pub fn empty_feedback(text: &str) -> FeedbackResult {
    FeedbackResult {
        original: text.to_string(),
        corrected: None,
        better_expression: None,
        idiom_or_vocab: None,
        explanation: None,
        has_error: false,
    }
}

/// 비교용 정규화: 소문자 + 글자/숫자/아포스트로피만 남기고 공백 정리
/// "we don't use it" vs "We don't use it." 같은 "교정"을 걸러내는 데 사용
fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' || c == '’' {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .replace('’', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "null")
}

/// LLM 출력 정리 - 작은 모델(8B)이 규칙을 어겨도 의미 없는 교정이 리포트에
/// 올라오지 않도록 코드에서 한 번 더 거른다.
pub fn sanitize(mut fb: FeedbackResult) -> FeedbackResult {
    let orig = normalize(&fb.original);

    fb.corrected = non_empty(fb.corrected).filter(|c| normalize(c) != orig);
    fb.explanation = non_empty(fb.explanation);
    fb.idiom_or_vocab = non_empty(fb.idiom_or_vocab);
    fb.better_expression = non_empty(fb.better_expression).filter(|b| {
        let nb = normalize(b);
        nb != orig && fb.corrected.as_deref().map(normalize) != Some(nb)
    });

    // 교정문이 없으면 오류가 아니고, 오류가 아니라면 교정문/설명도 없어야 함
    fb.has_error = fb.has_error && fb.corrected.is_some();
    if !fb.has_error {
        fb.corrected = None;
        fb.explanation = None;
    }
    fb
}

/// 한 턴에 대한 영어 피드백 요청
///
/// * `text` - 피드백할 턴
/// * `before` / `after` - 앞뒤 턴 (문맥 파악용, 교정 대상 아님)
pub async fn get_feedback(text: &str, before: &[&str], after: &[&str]) -> Result<FeedbackResult> {
    // 너무 짧으면 LLM 호출 스킵
    if text.split_whitespace().count() < 3 {
        return Ok(empty_feedback(text));
    }

    let client = reqwest::Client::new();

    let mut context = String::new();
    if !before.is_empty() || !after.is_empty() {
        context.push_str("\n\nConversation context (for understanding only - do NOT correct these):\n");
        for b in before {
            context.push_str(&format!("(before) \"{b}\"\n"));
        }
        for a in after {
            context.push_str(&format!("(after) \"{a}\"\n"));
        }
    }

    let prompt = format!("{SYSTEM_PROMPT}{context}\n\nStudent: \"{text}\"\n");

    // Ollama API 호출
    // serde_json::json! 매크로: JSON 리터럴 생성
    let response = client
        .post(OLLAMA_URL)
        .json(&serde_json::json!({
            "model": OLLAMA_MODEL,
            "prompt": prompt,
            "stream": false,
            "format": "json",
            "options": {
                "temperature": 0.2,  // 낮을수록 일관된 출력
                "num_predict": 300   // 최대 토큰 수 (턴이 길어져서 여유 있게)
            }
        }))
        .send()
        .await
        .context("Could not connect to Ollama. Make sure `ollama serve` is running.")?;

    // 응답 JSON 파싱
    let body: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse the Ollama response")?;

    let response_text = body["response"]
        .as_str()
        .context("Ollama response has no 'response' field")?;

    // LLM이 반환한 JSON 문자열 → FeedbackResult 구조체
    let mut feedback: FeedbackResult = serde_json::from_str(response_text)
        .with_context(|| format!("Failed to parse feedback JSON:\n{response_text}"))?;

    // 원문 주입 (LLM 응답에 포함되지 않으므로)
    feedback.original = text.to_string();

    Ok(sanitize(feedback))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(original: &str, corrected: Option<&str>, better: Option<&str>) -> FeedbackResult {
        FeedbackResult {
            original: original.into(),
            corrected: corrected.map(Into::into),
            better_expression: better.map(Into::into),
            idiom_or_vocab: None,
            explanation: Some("x".into()),
            has_error: true,
        }
    }

    #[test]
    fn drops_punctuation_only_corrections() {
        let out = sanitize(fb("we don't use it", Some("We don't use it."), None));
        assert!(!out.has_error);
        assert!(out.corrected.is_none());
        assert!(out.explanation.is_none());

        let out = sanitize(fb("That's a great part.", Some("That's a great part."), None));
        assert!(!out.has_error);
    }

    #[test]
    fn keeps_real_corrections() {
        let out = sanitize(fb("There was a holidays.", Some("There were holidays."), None));
        assert!(out.has_error);
        assert_eq!(out.corrected.as_deref(), Some("There were holidays."));
    }

    #[test]
    fn drops_better_expression_identical_to_correction() {
        let out = sanitize(fb(
            "I got holidays.",
            Some("I have holidays."),
            Some("I have holidays"),
        ));
        assert!(out.better_expression.is_none());
    }
}
