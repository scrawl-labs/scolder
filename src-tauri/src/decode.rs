// decode.rs
// 업로드된 오디오 파일(wav/mp3/m4a)을 Whisper가 기대하는 16kHz 모노 PCM으로 디코딩
//
// 핵심 Rust 개념:
// - symphonia: 순수 Rust 오디오 디코딩 라이브러리 (포맷 자동 감지 + 코덱 디코딩)
// - SampleBuffer: 어떤 원본 샘플 포맷(u8/s16/f32/...)이든 f32 인터리브 배열로 통일

use crate::dsp;
use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Whisper가 기대하는 샘플레이트 (audio.rs의 상수와 동일한 값이지만, 이 모듈은
/// "파일 → PCM" 경계를 독립적으로 소유하므로 별도로 정의)
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// 오디오 파일(wav/mp3/m4a)을 16kHz 모노 f32 PCM으로 디코딩
///
/// # 에러
/// 파일을 열 수 없거나, 포맷을 인식할 수 없거나, 오디오 트랙이 없거나,
/// 디코딩된 샘플이 하나도 없을 때
pub fn decode_file_to_pcm16k(path: &Path) -> Result<Vec<f32>> {
    let file = File::open(path).with_context(|| format!("Could not open the file: {path:?}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("Unrecognized audio format (supported: wav, mp3, m4a)")?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("No audio track found"))?
        .clone();

    let track_id = track.id;
    let source_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("Could not read the sample rate"))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("Could not create the audio decoder")?;

    // 채널 수는 컨테이너 헤더만으로 알 수 없는 코덱이 있음 (예: m4a의 AAC는
    // 채널 정보가 AudioSpecificConfig 안에 있어서, 컨테이너 파싱 단계가 아니라
    // 디코더가 첫 패킷을 실제로 디코딩해야만 codec_params.channels가 아닌
    // 디코딩된 버퍼의 spec()에서 알 수 있음). 그래서 여기서 미리 요구하지 않고
    // 첫 프레임을 디코딩한 뒤 그 spec에서 얻는다.
    let mut channels: Option<u16> = None;

    // 인터리브된 f32 샘플을 여기 누적 - SampleBuffer가 원본 포맷(u8/s16/f32/...)
    // 무엇이든 f32로 자동 변환해줘서 포맷별 분기 없이 처리 가능
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 파일 끝 - 정상 종료
            Err(SymphoniaError::IoError(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(e).context("Failed to read audio packets"),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded: AudioBufferRef = match decoder.decode(&packet) {
            Ok(d) => d,
            // 개별 패킷 손상은 건너뛰고 계속 - 파일 전체를 실패시키지 않음
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e).context("Failed to decode audio"),
        };

        if sample_buf.is_none() {
            let spec = *decoded.spec();
            let duration = decoded.capacity() as u64;
            channels = Some(spec.channels.count() as u16);
            sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
        }

        if let Some(buf) = &mut sample_buf {
            buf.copy_interleaved_ref(decoded);
            samples.extend_from_slice(buf.samples());
        }
    }

    if samples.is_empty() {
        return Err(anyhow!(
            "오디오 데이터를 읽지 못했습니다 (빈 파일이거나 지원하지 않는 코덱)"
        ));
    }
    // channels는 sample_buf/samples와 같은 시점에 함께 채워지므로, samples가
    // 비어있지 않다면 channels도 항상 Some
    let channels = channels.expect("samples가 비어있지 않으면 channels도 반드시 Some");

    let mono = dsp::to_mono(&samples, channels);
    Ok(dsp::resample(&mono, source_rate, WHISPER_SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트용 440Hz 사인파 wav 파일을 임시 경로에 생성 (hound는 이미 프로젝트
    /// 의존성이라 추가 크레이트 없이 픽스처를 즉석에서 만들 수 있음)
    fn write_test_wav(path: &Path, sample_rate: u32, channels: u16, seconds: f32) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let total_samples = (sample_rate as f32 * seconds) as usize;
        for i in 0..total_samples {
            let t = i as f32 / sample_rate as f32;
            let value = (t * 440.0 * std::f32::consts::TAU).sin();
            let amplitude = (value * i16::MAX as f32) as i16;
            for _ in 0..channels {
                writer.write_sample(amplitude).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn decodes_mono_wav_to_16k_pcm() {
        let dir = std::env::temp_dir();
        let path = dir.join("wtf_english_test_mono.wav");
        write_test_wav(&path, 16_000, 1, 0.5);

        let pcm = decode_file_to_pcm16k(&path).unwrap();

        // 0.5초 @ 16kHz ≈ 8000 샘플 (리샘플링 없음이므로 정확히 일치)
        assert_eq!(pcm.len(), 8_000);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn decodes_and_resamples_stereo_48k_wav() {
        let dir = std::env::temp_dir();
        let path = dir.join("wtf_english_test_stereo_48k.wav");
        write_test_wav(&path, 48_000, 2, 0.5);

        let pcm = decode_file_to_pcm16k(&path).unwrap();

        // 48kHz → 16kHz는 1/3 길이, 0.5초 @ 48kHz = 24000 샘플 → 8000 샘플
        assert_eq!(pcm.len(), 8_000);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn errors_on_missing_file() {
        let result = decode_file_to_pcm16k(Path::new("/nonexistent/path/does-not-exist.wav"));
        assert!(result.is_err());
    }

    /// 회귀 테스트: m4a(AAC)는 컨테이너 헤더만으로 채널 수를 알 수 없어서,
    /// 디코더가 첫 패킷을 실제로 디코딩하기 전에 channels를 요구하면
    /// "채널 정보를 읽을 수 없습니다" 에러로 항상 실패했었음.
    #[test]
    fn decodes_m4a_without_upfront_channel_metadata() {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test-tone.m4a"));
        let pcm = decode_file_to_pcm16k(path).unwrap();
        assert!(!pcm.is_empty());
    }
}
