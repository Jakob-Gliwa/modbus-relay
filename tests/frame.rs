use modbus_relay::frame::{Frame, FrameReader, MAX_ADU, crc16, read_rtu, rtu_request};
use proptest::prelude::*;
use tokio::io::{AsyncWriteExt, duplex};

#[tokio::test]
async fn fragmented_and_coalesced_frames() {
    let a = Frame::new(42, 1, &[3, 0, 0, 0, 2]).unwrap();
    let b = Frame::new(12, 8, &[6, 0, 1, 0, 9]).unwrap();
    let expected = [a.as_bytes().to_vec(), b.as_bytes().to_vec()];
    let (mut tx, mut rx) = duplex(64);
    let writer = tokio::spawn(async move {
        for byte in a.as_bytes() {
            tx.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        tx.write_all(b.as_bytes()).await.unwrap();
    });
    for bytes in expected {
        assert_eq!(
            Frame::read(&mut rx, false).await.unwrap().0.as_bytes(),
            bytes
        );
    }
    writer.await.unwrap();
}

#[tokio::test]
async fn oversized_zero_short_and_non_modbus_headers_rejected() {
    for length in [0_u16, 1, 255, 65535] {
        let mut bytes = [0, 1, 0, 0, 0, 0, 1, 3];
        bytes[4..6].copy_from_slice(&length.to_be_bytes());
        assert!(Frame::read(&mut bytes.as_slice(), false).await.is_err());
    }
    assert!(
        Frame::read(&mut &[0, 1, 0, 1, 0, 2, 1, 3][..], false)
            .await
            .is_err()
    );
    for len in 0..12 {
        let request = [0, 1, 0, 0, 0, 6, 1, 3, 0, 0, 0, 2];
        assert!(Frame::read(&mut &request[..len], false).await.is_err());
    }
}

#[tokio::test]
async fn exception_repair_is_response_only() {
    for length in [2_u8, 3, 4, 254] {
        let bytes = [0, 1, 0, 0, 0, length, 1, 0x83, 2];
        let (frame, repaired) = Frame::read(&mut bytes.as_slice(), true).await.unwrap();
        assert_eq!(frame.as_bytes(), &[0, 1, 0, 0, 0, 3, 1, 0x83, 2]);
        assert_eq!(repaired, length != 3);
    }
    assert!(
        Frame::read(&mut &[0, 1, 0, 0, 0, 4, 1, 0x83, 2][..], false)
            .await
            .is_err()
    );
}

#[test]
fn response_identity_and_exception() {
    let request = Frame::new(42, 2, &[3, 0, 0, 0, 1]).unwrap();
    assert!(request.exception(2).validate_response(&request).is_ok());
    for response in [
        Frame::new(43, 2, &[3, 2, 0, 1]).unwrap(),
        Frame::new(42, 3, &[3, 2, 0, 1]).unwrap(),
        Frame::new(42, 2, &[4, 2, 0, 1]).unwrap(),
    ] {
        assert!(response.validate_response(&request).is_err());
    }
    assert_eq!(
        request.exception(11).as_bytes(),
        &[0, 42, 0, 0, 0, 3, 2, 0x83, 11]
    );
}

#[test]
fn canonical_crc_vector() {
    assert_eq!(crc16(b"123456789"), 0x4b37);
    let (bytes, len) = rtu_request(&Frame::new(7, 1, &[3, 0, 0, 0, 10]).unwrap());
    assert_eq!(&bytes[..len], &[1, 3, 0, 0, 0, 10, 0xc5, 0xcd]);
}

#[tokio::test]
async fn rtu_fixed_variable_exception_crc_and_oversize() {
    for pdu in [
        vec![3, 2, 0, 10],
        vec![6, 0, 0, 0, 10],
        vec![0x83, 2],
        vec![22, 0, 1, 0, 0, 0, 1],
        vec![17, 2, 7, 8],
    ] {
        let frame = Frame::new(7, 1, &pdu).unwrap();
        let (bytes, len) = rtu_request(&frame);
        assert_eq!(
            read_rtu(&mut &bytes[..len], 7).await.unwrap().as_bytes(),
            frame.as_bytes()
        );
        let mut corrupt = bytes;
        corrupt[len - 1] ^= 1;
        assert!(read_rtu(&mut &corrupt[..len], 7).await.is_err());
    }
    assert!(read_rtu(&mut &[1, 3, 252][..], 7).await.is_err());
    assert!(read_rtu(&mut &[1, 65, 0][..], 7).await.is_err());
}

#[tokio::test]
async fn rtu_does_not_split_on_accidental_crc_prefix() {
    let mut pdu = vec![3, 6, 0x12];
    let prefix_crc = crc16(&[1, 3, 6, 0x12]);
    pdu.extend_from_slice(&prefix_crc.to_le_bytes());
    pdu.extend_from_slice(&[0x22, 0x33, 0x44]);
    let frame = Frame::new(1, 1, &pdu).unwrap();
    let (bytes, len) = rtu_request(&frame);
    assert_eq!(read_rtu(&mut &bytes[..len], 1).await.unwrap().pdu(), pdu);
}

proptest! {
    #[test]
    fn buffered_parser_matches_exact_parser(bytes in prop::collection::vec(any::<u8>(),0..=600)) {
        let runtime=tokio::runtime::Builder::new_current_thread().build().unwrap();
        for response in [false,true] {
            let exact=runtime.block_on(Frame::read(&mut bytes.as_slice(),response));
            let buffered=runtime.block_on(FrameReader::default().read(&mut bytes.as_slice(),response));
            match (exact,buffered) {
                (Ok((a,ar)),Ok(Some((b,br))))=>{prop_assert_eq!(a.as_bytes(),b.as_bytes());prop_assert_eq!(ar,br);},
                (Err(a),Err(b))=>prop_assert_eq!(a.kind(),b.kind()),
                (Err(_),Ok(None))=>prop_assert!(bytes.is_empty()),
                other=>prop_assert!(false,"parser disagreement: {other:?}"),
            }
        }
    }
    #[test]
    fn arbitrary_standard_pdus_never_panic(
        function in prop::sample::select(vec![1_u8,2,3,4,5,6,15,16,17,22,23,65]),
        input in prop::collection::vec(any::<u8>(),0..=252),
        output in prop::collection::vec(any::<u8>(),0..=252),
    ) {
        let mut pdu=vec![function]; pdu.extend_from_slice(&input);
        let request=Frame::new(1,1,&pdu).unwrap();
        let _=request.validate_request();
        let mut pdu=vec![function]; pdu.extend_from_slice(&output);
        let response=Frame::new(1,1,&pdu).unwrap();
        let _=response.validate_response(&request);
    }
    #[test]
    fn all_valid_adu_sizes_roundtrip(tid in any::<u16>(), unit in any::<u8>(), pdu in prop::collection::vec(any::<u8>(),1..=253)) {
        let frame = Frame::new(tid,unit,&pdu).unwrap();
        prop_assert_eq!(frame.as_bytes().len(),7+pdu.len());
        let runtime=tokio::runtime::Builder::new_current_thread().build().unwrap();
        let decoded=runtime.block_on(Frame::read(&mut frame.as_bytes(),false)).unwrap().0;
        prop_assert_eq!(decoded.as_bytes(),frame.as_bytes());
        let (rtu,len)=rtu_request(&frame);
        prop_assert_eq!(crc16(&rtu[..len]),0);
    }
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(),0..=600)) {
        let runtime=tokio::runtime::Builder::new_current_thread().build().unwrap();
        for response in [false,true] { let _=runtime.block_on(Frame::read(&mut bytes.as_slice(),response)); }
        let _=runtime.block_on(read_rtu(&mut bytes.as_slice(),0));
    }
}

struct ChunkedReader {
    bytes: Vec<u8>,
    position: usize,
    chunk: usize,
    reads: usize,
}
impl tokio::io::AsyncRead for ChunkedReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        assert!(
            buf.remaining() <= MAX_ADU,
            "receive buffer became unbounded"
        );
        self.reads += 1;
        let size = buf
            .remaining()
            .min(self.chunk)
            .min(self.bytes.len() - self.position);
        buf.put_slice(&self.bytes[self.position..self.position + size]);
        self.position += size;
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn buffered_reader_preserves_pipeline_and_reduces_read_calls() {
    let frames = [
        Frame::new(1, 1, &[3, 0, 0, 0, 1]).unwrap(),
        Frame::new(2, 1, &[6, 0, 1, 0, 7]).unwrap(),
    ];
    let bytes = frames
        .iter()
        .flat_map(|f| f.as_bytes().iter().copied())
        .collect::<Vec<_>>();
    let mut source = ChunkedReader {
        bytes: bytes.clone(),
        position: 0,
        chunk: MAX_ADU,
        reads: 0,
    };
    let mut reader = FrameReader::default();
    for frame in &frames {
        assert_eq!(
            reader
                .read(&mut source, false)
                .await
                .unwrap()
                .unwrap()
                .0
                .as_bytes(),
            frame.as_bytes()
        );
    }
    assert_eq!(source.reads, 1, "two coalesced frames should need one read");
    assert!(reader.read(&mut source, false).await.unwrap().is_none());
    let mut source = ChunkedReader {
        bytes,
        position: 0,
        chunk: 1,
        reads: 0,
    };
    for frame in &frames {
        assert_eq!(
            reader
                .read(&mut source, false)
                .await
                .unwrap()
                .unwrap()
                .0
                .as_bytes(),
            frame.as_bytes()
        );
    }
}

#[tokio::test]
async fn buffered_reader_compacts_partial_maximum_frame_and_repairs_exceptions() {
    let a = Frame::new(1, 1, &[65, 1]).unwrap();
    let b = Frame::new(2, 1, &[65; 253]).unwrap();
    let bytes = [a.as_bytes(), b.as_bytes(), a.as_bytes()].concat();
    let mut source = ChunkedReader {
        bytes,
        position: 0,
        chunk: MAX_ADU,
        reads: 0,
    };
    let mut reader = FrameReader::default();
    for frame in [&a, &b, &a] {
        assert_eq!(
            reader
                .read(&mut source, false)
                .await
                .unwrap()
                .unwrap()
                .0
                .as_bytes(),
            frame.as_bytes()
        );
    }
    for length in [2_u8, 3, 4, 254] {
        let bytes = [0, 1, 0, 0, 0, length, 1, 0x83, 2];
        let (frame, repaired) = FrameReader::default()
            .read(&mut bytes.as_slice(), true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.as_bytes(), &[0, 1, 0, 0, 0, 3, 1, 0x83, 2]);
        assert_eq!(repaired, length != 3);
    }
    for len in 1..b.as_bytes().len() {
        let error = FrameReader::default()
            .read(&mut &b.as_bytes()[..len], false)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}

#[test]
fn standard_request_quantity_address_and_byte_count_boundaries() {
    for (function, max) in [
        (1_u8, 2000_u16),
        (2, 2000),
        (3, 125),
        (4, 125),
        (15, 1968),
        (16, 123),
    ] {
        for quantity in [0_u16, 1, max, max + 1, u16::MAX] {
            let mut pdu = vec![function, 0, 0, (quantity >> 8) as u8, quantity as u8];
            if function >= 15 {
                let bytes = if function == 15 {
                    usize::from(quantity).div_ceil(8)
                } else {
                    usize::from(quantity) * 2
                };
                // Invalid large quantities still exercise overflow checks with bounded input.
                let bytes = bytes.min(247);
                pdu.push(bytes as u8);
                pdu.resize(6 + bytes, 0);
            }
            let valid = quantity > 0 && quantity <= max;
            assert_eq!(
                Frame::new(1, 1, &pdu).unwrap().validate_request().is_ok(),
                valid,
                "{pdu:?}"
            );
            if valid {
                // Boundary address is valid only for exactly one element.
                pdu[1..3].copy_from_slice(&u16::MAX.to_be_bytes());
                assert_eq!(
                    Frame::new(1, 1, &pdu).unwrap().validate_request(),
                    if quantity == 1 { Ok(()) } else { Err(2) }
                );
            }
        }
    }
    // libmodbus #887: FC15 requires exactly ceil(quantity/8), not arbitrary padding.
    for pdu in [
        vec![15, 0, 0, 0, 9, 1, 0],
        vec![15, 0, 0, 0, 8, 2, 0, 0],
        vec![16, 0, 0, 0, 1, 1, 0],
    ] {
        assert_eq!(Frame::new(1, 1, &pdu).unwrap().validate_request(), Err(3));
    }
    for (pdu, valid) in [
        (vec![5, 0, 0, 0xff, 0], true),
        (vec![5, 0, 0, 0, 0], true),
        (vec![5, 0, 0, 0, 1], false),
        (vec![6, 0, 0, 0xff, 0xff], true),
        (vec![17], true),
        (vec![17, 0], false),
        (vec![22, 0, 1, 0, 0, 0, 1], true),
        (vec![22, 0, 1, 0, 0, 0], false),
        (vec![23, 0, 0, 0, 125, 0, 0, 0, 1, 2, 0, 1], true),
        (vec![23, 0, 0, 0, 126, 0, 0, 0, 1, 2, 0, 1], false),
        (vec![23, 0, 0, 0, 1, 0, 0, 0, 122, 2, 0, 1], false),
        (vec![23, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0], false),
    ] {
        assert_eq!(
            Frame::new(1, 1, &pdu).unwrap().validate_request().is_ok(),
            valid,
            "{pdu:?}"
        );
    }
    let mut max_fc23 = vec![23, 0, 0, 0, 125, 0, 0, 0, 121, 242];
    max_fc23.resize(252, 0xff);
    assert!(
        Frame::new(1, 1, &max_fc23)
            .unwrap()
            .validate_request()
            .is_ok()
    );
    for function in [1, 2, 3, 4, 5, 6, 15, 16, 22, 23] {
        assert_eq!(
            Frame::new(1, 1, &[function]).unwrap().validate_request(),
            Err(3)
        );
    }
}

#[test]
fn standard_responses_validate_counts_and_every_write_echo_field() {
    for pdu in [
        vec![5, 0, 1, 0xff, 0],
        vec![6, 0, 1, 0, 7],
        vec![15, 0, 1, 0, 9, 2, 0, 0],
        vec![16, 0, 1, 0, 1, 2, 0, 7],
        vec![22, 0, 1, 0xff, 0, 0, 7],
    ] {
        let request = Frame::new(1, 1, &pdu).unwrap();
        let ack = if matches!(pdu[0], 15 | 16) {
            &pdu[..5]
        } else {
            &pdu[..]
        };
        assert!(
            Frame::new(1, 1, ack)
                .unwrap()
                .validate_response(&request)
                .is_ok()
        );
        for index in 1..ack.len() {
            let mut corrupt = ack.to_vec();
            corrupt[index] ^= 1;
            assert!(
                Frame::new(1, 1, &corrupt)
                    .unwrap()
                    .validate_response(&request)
                    .is_err()
            );
        }
    }
    for function in [1, 2, 3, 4, 23] {
        let pdu = if function == 23 {
            vec![23, 0, 0, 0, 1, 0, 0, 0, 1, 2, 0, 1]
        } else {
            vec![function, 0, 0, 0, 1]
        };
        let req = Frame::new(1, 1, &pdu).unwrap();
        let expected = if function <= 2 { 1 } else { 2 };
        let mut valid = vec![function, expected];
        valid.resize(2 + usize::from(expected), 0);
        assert!(
            Frame::new(1, 1, &valid)
                .unwrap()
                .validate_response(&req)
                .is_ok()
        );
        for invalid in [
            vec![function],
            vec![function, 0],
            vec![function, 1, 0, 0],
            vec![function, 3, 0, 0, 0],
        ] {
            assert!(
                Frame::new(1, 1, &invalid)
                    .unwrap()
                    .validate_response(&req)
                    .is_err()
            );
        }
    }
    let req = Frame::new(1, 1, &[17]).unwrap();
    assert!(
        Frame::new(1, 1, &[17, 2, 7, 0xff])
            .unwrap()
            .validate_response(&req)
            .is_ok()
    );
    for pdu in [vec![17], vec![17, 1, 7], vec![17, 3, 7, 0xff]] {
        assert!(
            Frame::new(1, 1, &pdu)
                .unwrap()
                .validate_response(&req)
                .is_err()
        );
    }
    let vendor = Frame::new(1, 1, &[0x41, 99]).unwrap();
    assert!(vendor.validate_request().is_ok());
    assert!(vendor.exception(0x80).validate_response(&vendor).is_ok());
    assert!(vendor.exception(0).validate_response(&vendor).is_err());
    assert!(
        Frame::new(1, 1, &[0xc1, 1, 0])
            .unwrap()
            .validate_response(&vendor)
            .is_err()
    );
    assert!(
        Frame::new(1, 1, &[0xc1])
            .unwrap()
            .validate_response(&vendor)
            .is_err()
    );
}
