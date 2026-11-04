// audio.rs
// 마이크 입력을 캡처하고 VAD(Voice Activity Detection)로 발화 단위 분리
//
// 핵심 Rust 개념:
// - 클로저 (Closure): move 클로저로 외부 변수 소유권 이동
// - mpsc 채널: 스레드 간 데이터 전달
// - cpal: 크로스플랫폼 오디오 I/O 라이브러리

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;
use crate::dsp::{resample, to_mono};

/// Whisper가 기대하는 샘플레이트
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// RMS 에너지 기반 VAD 임계값
/// 값이 낮을수록 민감 (조용한 환경: 0.005, 시끄러운 환경: 0.02)
const SILENCE_THRESHOLD: f32 = 0.01;

/// 발화 종료 판단 기준: 침묵이 이 개수만큼 연속되면 한 발화 끝
/// 16000Hz / 512샘플 청크 ≈ 초당 31 청크 → 16청크 ≈ 0.5초
const SILENCE_CHUNKS_TO_END: usize = 16;

/// 최소 발화 길이 (샘플 수) - 너무 짧은 잡음 무시
/// 0.5초 = 8000 샘플
const MIN_SPEECH_SAMPLES: usize = 8_000;

/// 마이크 캡처 시작
///
/// # 반환값
/// `cpal::Stream` - 이 값을 살려두는 동안 캡처가 계속됨. drop하면 중단.
///
/// # 에러
/// 마이크 장치가 없거나 스트림 생성 실패 시
pub fn start_capture(tx: mpsc::SyncSender<Vec<f32>>) -> anyhow::Result<cpal::Stream> {
    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No microphone found"))?;

    let supported_config = device.default_input_config()?;
    let channels = supported_config.channels();
    let device_sample_rate = supported_config.sample_rate().0;

    println!(
        "[Audio] 마이크: {} / {}Hz / {}채널",
        device.name().unwrap_or_default(),
        device_sample_rate,
        channels
    );

    let config = supported_config.into();

    // VAD 상태값들 - move 클로저가 소유권을 가져감
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut silence_chunk_count: usize = 0;
    let mut is_speaking = false;

    let stream = device.build_input_stream(
        &config,
        // move: 클로저가 외부 변수들의 소유권을 가져옴
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            // 1) 멀티채널 → 모노 변환
            let mono = to_mono(data, channels);

            // 2) 샘플레이트 변환 (기기 Hz → 16kHz)
            let samples = resample(&mono, device_sample_rate, WHISPER_SAMPLE_RATE);

            // 3) VAD: RMS 에너지로 발화 감지
            let rms = calculate_rms(&samples);

            if rms > SILENCE_THRESHOLD {
                // 발화 중
                if !is_speaking {
                    println!("[Audio] 발화 시작 (RMS: {rms:.4})");
                }
                is_speaking = true;
                silence_chunk_count = 0;
                speech_buffer.extend_from_slice(&samples);
            } else if is_speaking {
                // 발화 후 침묵
                speech_buffer.extend_from_slice(&samples);
                silence_chunk_count += 1;

                if silence_chunk_count >= SILENCE_CHUNKS_TO_END {
                    // 발화 종료 판정
                    if speech_buffer.len() >= MIN_SPEECH_SAMPLES {
                        println!(
                            "[Audio] 발화 종료 - {}ms 분량 전송",
                            speech_buffer.len() * 1000 / WHISPER_SAMPLE_RATE as usize
                        );
                        // try_send: 채널이 꽉 차도 블로킹하지 않음
                        if let Err(e) = tx.try_send(speech_buffer.clone()) {
                            eprintln!("[Audio] 채널 전송 실패: {e}");
                        }
                    }
                    speech_buffer.clear();
                    silence_chunk_count = 0;
                    is_speaking = false;
                }
            }
        },
        |err| eprintln!("[Audio] 스트림 에러: {err}"),
        None,
    )?;

    stream.play()?;
    Ok(stream)
}

/// RMS (Root Mean Square) - 오디오 신호의 평균 에너지
fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_square = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    mean_square.sqrt()
}
