//! Four-byte big-endian length-prefixed framing. Mirrors `packages/protocol/src/framing.ts`.

use std::fmt;

const FRAME_HEADER_LENGTH: usize = 4;
const PAYLOAD_BLOCK_SIZE: usize = 64 * 1024;
const MAX_UINT32: u64 = 0xffff_ffff;

/// Default upper bound for one framed CBOR payload.
pub const DEFAULT_MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameError(pub String);

impl fmt::Display for FrameError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for FrameError {}

/// Prefixes a payload with its unsigned 32-bit big-endian byte length.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
	if payload.len() as u64 > MAX_UINT32 {
		return Err(FrameError(
			"Frame payload exceeds the unsigned 32-bit length limit".into(),
		));
	}
	let mut frame = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
	frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
	frame.extend_from_slice(payload);
	Ok(frame)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	Open,
	Ended,
	Failed,
}

/// Incrementally splits arbitrary byte chunks into length-prefixed payloads.
///
/// Payload memory grows with the bytes actually received, never with the declared length alone.
#[derive(Debug)]
pub struct FrameDecoder {
	header: [u8; FRAME_HEADER_LENGTH],
	header_length: usize,
	max_frame_length: usize,
	payload: Vec<u8>,
	expected_payload_length: Option<usize>,
	state: State,
}

impl FrameDecoder {
	pub fn new(max_frame_length: usize) -> Self {
		Self {
			header: [0; FRAME_HEADER_LENGTH],
			header_length: 0,
			max_frame_length,
			payload: Vec::new(),
			expected_payload_length: None,
			state: State::Open,
		}
	}

	fn check_open(&self) -> Result<(), FrameError> {
		match self.state {
			State::Open => Ok(()),
			State::Ended => Err(FrameError("Frame decoder has ended".into())),
			State::Failed => Err(FrameError("Frame decoder has failed".into())),
		}
	}

	pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
		self.check_open()?;
		let mut frames = Vec::new();
		let mut offset = 0;
		while offset < chunk.len() {
			let expected = match self.expected_payload_length {
				Some(expected) => expected,
				None => {
					let header_bytes = (FRAME_HEADER_LENGTH - self.header_length).min(chunk.len() - offset);
					self.header[self.header_length..self.header_length + header_bytes]
						.copy_from_slice(&chunk[offset..offset + header_bytes]);
					self.header_length += header_bytes;
					offset += header_bytes;
					if self.header_length < FRAME_HEADER_LENGTH {
						continue;
					}
					let frame_length = u32::from_be_bytes(self.header) as usize;
					self.header_length = 0;
					if frame_length > self.max_frame_length {
						return Err(self.fail(format!(
							"Frame length {frame_length} exceeds configured limit of {}",
							self.max_frame_length
						)));
					}
					if frame_length == 0 {
						frames.push(Vec::new());
						continue;
					}
					self.expected_payload_length = Some(frame_length);
					self.payload = Vec::with_capacity(frame_length.min(PAYLOAD_BLOCK_SIZE));
					frame_length
				}
			};

			let payload_bytes = (expected - self.payload.len()).min(chunk.len() - offset);
			self.payload.extend_from_slice(&chunk[offset..offset + payload_bytes]);
			offset += payload_bytes;
			if self.payload.len() == expected {
				frames.push(std::mem::take(&mut self.payload));
				self.expected_payload_length = None;
			}
		}
		Ok(frames)
	}

	pub fn end(&mut self) -> Result<(), FrameError> {
		self.check_open()?;
		if self.header_length != 0 || self.expected_payload_length.is_some() {
			return Err(self.fail("Truncated frame at end of stream".into()));
		}
		self.state = State::Ended;
		Ok(())
	}

	fn fail(&mut self, message: String) -> FrameError {
		self.state = State::Failed;
		self.header_length = 0;
		self.payload = Vec::new();
		self.expected_payload_length = None;
		FrameError(message)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn decode_all(decoder: &mut FrameDecoder, chunks: &[&[u8]]) -> Vec<Vec<u8>> {
		chunks.iter().flat_map(|chunk| decoder.push(chunk).unwrap()).collect()
	}

	#[test]
	fn prefixes_payloads_with_a_big_endian_length() {
		assert_eq!(
			encode_frame(&[0xaa, 0xbb, 0xcc]).unwrap(),
			[0, 0, 0, 3, 0xaa, 0xbb, 0xcc]
		);
		assert_eq!(encode_frame(&[]).unwrap(), [0, 0, 0, 0]);
	}

	#[test]
	fn decodes_fragmented_coalesced_and_empty_frames_in_order() {
		let wire = [
			encode_frame(&[1, 2, 3]).unwrap(),
			encode_frame(&[]).unwrap(),
			encode_frame(&[4]).unwrap(),
		]
		.concat();
		let expected = vec![vec![1, 2, 3], vec![], vec![4]];

		let mut decoder = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
		let bytes: Vec<&[u8]> = wire.chunks(1).collect();
		assert_eq!(decode_all(&mut decoder, &bytes), expected);
		decoder.end().unwrap();

		let mut coalesced = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
		assert_eq!(coalesced.push(&wire).unwrap(), expected);
		coalesced.end().unwrap();
	}

	#[test]
	fn assembles_payloads_larger_than_one_block() {
		let payload: Vec<u8> = (0..70_000).map(|index| (index % 251) as u8).collect();
		let wire = encode_frame(&payload).unwrap();
		let mut decoder = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
		assert_eq!(
			decode_all(&mut decoder, &[&wire[..101], &wire[101..65_541], &wire[65_541..]]),
			vec![payload]
		);
		decoder.end().unwrap();
	}

	#[test]
	fn handles_every_split_point() {
		let wire = encode_frame(&[10, 20, 30, 40]).unwrap();
		for split in 0..=wire.len() {
			let mut decoder = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
			assert_eq!(
				decode_all(&mut decoder, &[&wire[..split], &wire[split..]]),
				vec![vec![10, 20, 30, 40]]
			);
			decoder.end().unwrap();
		}
	}

	#[test]
	fn rejects_truncated_streams() {
		for wire in [&[0u8, 0, 0][..], &[0, 0, 0, 2, 1][..]] {
			let mut decoder = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
			assert!(decoder.push(wire).unwrap().is_empty());
			assert_eq!(decoder.end().unwrap_err().0, "Truncated frame at end of stream");
		}
	}

	#[test]
	fn rejects_oversized_frames_and_stays_failed() {
		let mut decoder = FrameDecoder::new(3);
		assert_eq!(
			decoder.push(&[0, 0, 0, 4]).unwrap_err().0,
			"Frame length 4 exceeds configured limit of 3"
		);
		assert_eq!(decoder.push(&[1]).unwrap_err().0, "Frame decoder has failed");

		let mut exact = FrameDecoder::new(3);
		assert_eq!(
			exact.push(&encode_frame(&[1, 2, 3]).unwrap()).unwrap(),
			vec![vec![1, 2, 3]]
		);
		exact.end().unwrap();
	}

	#[test]
	fn cannot_be_used_after_end() {
		let mut decoder = FrameDecoder::new(DEFAULT_MAX_FRAME_LENGTH);
		decoder.end().unwrap();
		assert_eq!(decoder.push(&[]).unwrap_err().0, "Frame decoder has ended");
		assert_eq!(decoder.end().unwrap_err().0, "Frame decoder has ended");
	}
}
