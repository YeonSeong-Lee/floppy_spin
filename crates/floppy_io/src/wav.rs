//! Minimal RIFF/WAVE encoder for headless audio verification (SPEC §8, §12).

/// Encode mono 16-bit PCM samples as a canonical RIFF/WAVE file.
pub fn encode_mono16(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    const FMT_CHUNK_SIZE: u32 = 16;

    let data_len = (samples.len() * 2) as u32;
    let riff_size = 4 // "WAVE"
        + (8 + FMT_CHUNK_SIZE)
        + (8 + data_len);

    let block_align: u16 = CHANNELS * (BITS_PER_SAMPLE / 8);
    let byte_rate: u32 = sample_rate * block_align as u32;

    let mut out = Vec::with_capacity(44 + samples.len() * 2);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&FMT_CHUNK_SIZE.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat: PCM
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_and_sizes_for_four_samples() {
        let samples: [i16; 4] = [0, 1000, -1000, i16::MAX];
        let wav = encode_mono16(44_100, &samples);

        assert_eq!(&wav[0..4], b"RIFF");
        let riff_size = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_size as usize, wav.len() - 8);
        assert_eq!(&wav[8..12], b"WAVE");

        assert_eq!(&wav[12..16], b"fmt ");
        let fmt_size = u32::from_le_bytes(wav[16..20].try_into().unwrap());
        assert_eq!(fmt_size, 16);
        let audio_format = u16::from_le_bytes(wav[20..22].try_into().unwrap());
        assert_eq!(audio_format, 1); // PCM
        let channels = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        assert_eq!(channels, 1);
        let sample_rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        assert_eq!(sample_rate, 44_100);
        let byte_rate = u32::from_le_bytes(wav[28..32].try_into().unwrap());
        assert_eq!(byte_rate, 44_100 * 2);
        let block_align = u16::from_le_bytes(wav[32..34].try_into().unwrap());
        assert_eq!(block_align, 2);
        let bits_per_sample = u16::from_le_bytes(wav[34..36].try_into().unwrap());
        assert_eq!(bits_per_sample, 16);

        assert_eq!(&wav[36..40], b"data");
        let data_size = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_size, (samples.len() * 2) as u32);

        assert_eq!(wav.len(), 44 + samples.len() * 2);

        for (i, &s) in samples.iter().enumerate() {
            let off = 44 + i * 2;
            let got = i16::from_le_bytes(wav[off..off + 2].try_into().unwrap());
            assert_eq!(got, s);
        }
    }

    #[test]
    fn wav_empty_samples_has_zero_data_size() {
        let wav = encode_mono16(8_000, &[]);
        assert_eq!(wav.len(), 44);
        let data_size = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_size, 0);
    }
}
