// mic_permission.rs
// 마이크 권한(TCC)을 캡처 시작 *전에* 확인/요청한다.
//
// 왜 필요한가: cpal의 스트림 생성/재생은 권한 프롬프트가 떠 있는 동안 블록되고,
// 권한이 거부돼도 CoreAudio는 에러 대신 무음을 전달한다. 그래서 캡처 시작 타임아웃으로는
// "사용자가 아직 답하는 중"과 "실패"를 구분할 수 없다. AVFoundation으로 먼저 상태를 확인한다.

/// 권한 확인 결과
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicAccess {
    Authorized,
}

/// 마이크 권한이 거부/제한된 경우의 사용자 안내 메시지
pub const DENIED_MESSAGE: &str = "Microphone access is denied. Enable Scoldler in System Settings → Privacy & Security → Microphone, then try again.";

/// AVAuthorizationStatus 값을 해석한 다음 행동 (순수 함수 - 테스트 가능)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Proceed,
    Request,
    Deny,
}

/// AVAuthorizationStatus 원시값: 0=NotDetermined, 1=Restricted, 2=Denied, 3=Authorized
/// 알 수 없는 값은 보수적으로 거부 처리한다.
fn decide(raw_status: isize) -> Decision {
    match raw_status {
        3 => Decision::Proceed,
        0 => Decision::Request,
        _ => Decision::Deny,
    }
}

/// 사용자가 프롬프트에 응답한 결과 -> 최종 결과
fn resolve_request(granted: bool) -> Result<MicAccess, String> {
    if granted {
        Ok(MicAccess::Authorized)
    } else {
        Err(DENIED_MESSAGE.to_string())
    }
}

/// 마이크 권한을 확인하고, 미결정이면 시스템 프롬프트를 띄워 사용자의 응답을 기다린다.
/// 응답 대기에는 타임아웃이 없다 (사용자가 오래 걸릴 수 있음).
#[cfg(target_os = "macos")]
pub async fn ensure_microphone_access() -> Result<MicAccess, String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let media_type = unsafe { AVMediaTypeAudio }
        .ok_or_else(|| "AVFoundation audio media type is unavailable".to_string())?;

    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    match decide(status.0) {
        Decision::Proceed => return Ok(MicAccess::Authorized),
        Decision::Deny => return Err(DENIED_MESSAGE.to_string()),
        Decision::Request => {}
    }

    // 완료 블록은 임의의 디스패치 큐에서 한 번 호출됨 -> oneshot으로 tokio 쪽에 전달
    // 블록은 Fn이어야 하므로 Sender를 Mutex<Option>으로 감싸 한 번만 꺼내 쓴다.
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = std::sync::Mutex::new(Some(tx));
    // RcBlock은 Send가 아니므로 await 이전에 스코프를 닫아 drop한다.
    // (AVFoundation이 블록을 copy/retain 하므로 호출 전까지 유효)
    {
        let handler = RcBlock::new(move |granted: Bool| {
            if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                let _ = tx.send(granted.as_bool());
            }
        });
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler);
        }
    }

    // 블록이 호출되지 않고 drop되면(sender drop) 거부로 간주
    let granted = rx.await.unwrap_or(false);
    resolve_request(granted)
}

/// macOS 외 플랫폼은 별도 권한 절차가 없으므로 항상 허용
#[cfg(not(target_os = "macos"))]
pub async fn ensure_microphone_access() -> Result<MicAccess, String> {
    Ok(MicAccess::Authorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorized_proceeds() {
        assert_eq!(decide(3), Decision::Proceed);
    }

    #[test]
    fn not_determined_requests() {
        assert_eq!(decide(0), Decision::Request);
    }

    #[test]
    fn denied_and_restricted_are_denied() {
        assert_eq!(decide(2), Decision::Deny);
        assert_eq!(decide(1), Decision::Deny);
    }

    #[test]
    fn unknown_status_is_denied() {
        assert_eq!(decide(99), Decision::Deny);
        assert_eq!(decide(-1), Decision::Deny);
    }

    #[test]
    fn request_result_mapping() {
        assert_eq!(resolve_request(true), Ok(MicAccess::Authorized));
        let err = resolve_request(false).unwrap_err();
        assert!(err.contains("System Settings"));
        assert!(err.contains("Microphone"));
    }
}
