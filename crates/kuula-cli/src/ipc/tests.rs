use super::*;

fn round_trip(msg: Message) {
    let bytes = encode(&msg);
    let mut cursor = std::io::Cursor::new(bytes);
    let back = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(back, msg);
    assert_eq!(cursor.position() as usize, cursor.get_ref().len());
}

#[test]
fn every_message_round_trips() {
    let snap = Snapshot::from_entries(
        [("main.lua", b"x".to_vec()), ("gfx/a.png", vec![1, 2, 3])],
        SnapshotLimits::default(),
    )
    .unwrap();
    round_trip(Message::Load {
        snapshot: snap,
        saves: vec![(0, b"a".to_vec()), (7, vec![0; SLOT_BYTES])],
        net: NetEnv {
            permitted: true,
            invite: Some("endpointabc".into()),
        },
    });
    round_trip(Message::Load {
        snapshot: Snapshot::empty(),
        saves: Vec::new(),
        net: NetEnv::default(),
    });
    round_trip(Message::Step {
        input: FrameInput::new(0b101),
        events: Vec::new(),
    });
    round_trip(Message::Step {
        input: FrameInput::NONE,
        events: vec![
            Event::Hosting {
                ticket: "t".repeat(MAX_TICKET),
            },
            Event::Connected { peer: 1 },
            Event::Message {
                from: 1,
                data: vec![0, 255, 7],
            },
            Event::Message {
                from: 1,
                data: vec![9; MAX_DATA],
            },
            Event::Disconnected {
                reason: Reason::Lost,
            },
            Event::Failed {
                code: FailCode::QueueFull,
                detail: "full".into(),
            },
            Event::Permission { granted: false },
        ],
    });
    round_trip(Message::Stop);
    round_trip(Message::Ready {
        width: 320,
        height: 240,
    });
    let mut palette = [[0u8; 3]; PALETTE_SIZE];
    palette[3] = [1, 2, 3];
    let plain = |frame: OwnedFrame| Message::Frame {
        frame,
        saves: Vec::new(),
        commands: Vec::new(),
        net_room: 0,
    };
    round_trip(Message::Frame {
        frame: OwnedFrame {
            frame: 7,
            width: 2,
            height: 2,
            pixels: vec![1, 2, 3, 4],
            palette,
            profile: FrameProfile::default(),
            audio: vec![1, -2, 32767, -32768],
            log: vec!["a".into(), "".into()],
            state: ConsoleState::Running,
        },
        saves: vec![(3, vec![9; 10])],
        commands: vec![
            Command::Host,
            Command::Join {
                ticket: "endpointx".into(),
            },
            Command::Send { data: vec![1, 2] },
            Command::Leave,
        ],
        net_room: MAX_BATCH as u32,
    });
    round_trip(plain(OwnedFrame {
        frame: 8,
        width: 1,
        height: 1,
        pixels: vec![0],
        palette,
        profile: FrameProfile::default(),
        log: vec![],
        audio: vec![],
        state: ConsoleState::Faulted(Fault::new("runtime_error", "main.lua", Some(3), "boom")),
    }));
    round_trip(plain(OwnedFrame {
        frame: 9,
        width: 1,
        height: 1,
        pixels: vec![0],
        palette,
        profile: FrameProfile::default(),
        log: vec![],
        audio: vec![],
        state: ConsoleState::Faulted(Fault::new("cart_read_error", "main.lua", None, "")),
    }));
    round_trip(Message::Error {
        code: "worker_error".into(),
        message: "no".into(),
    });
    // The profile travels with the frame.
    round_trip(plain(OwnedFrame {
        frame: 10,
        width: 1,
        height: 1,
        pixels: vec![0],
        palette,
        log: vec![],
        state: ConsoleState::Running,
        profile: FrameProfile {
            cycles: [1, 2, 3, 4, 5, 6, u64::MAX],
            budget: 279_620,
            lua_mem: 12345,
        },
        audio: vec![0; MAX_AUDIO_VALUES],
    }));
}

#[test]
fn long_fault_strings_are_clipped_not_refused() {
    // A cart can raise any message; it must still arrive as a fault.
    let long = "ä".repeat(3000); // 6000 bytes, clipped on a char boundary
    let msg = Message::Frame {
        frame: OwnedFrame {
            frame: 1,
            width: 1,
            height: 1,
            pixels: vec![0],
            palette: [[0; 3]; PALETTE_SIZE],
            profile: FrameProfile::default(),
            log: vec![],
            audio: vec![],
            state: ConsoleState::Faulted(Fault::new(
                "runtime_error",
                &"f".repeat(300),
                Some(3),
                &long,
            )),
        },
        saves: Vec::new(),
        commands: Vec::new(),
        net_room: 0,
    };
    let bytes = encode(&msg);
    let back = decode(&bytes[4..]).unwrap();
    let Message::Frame { frame: f, .. } = back else {
        panic!("not a frame")
    };
    let ConsoleState::Faulted(fault) = f.state else {
        panic!("not faulted")
    };
    assert_eq!(fault.message.len(), MAX_MESSAGE_BYTES);
    assert_eq!(fault.file.len(), MAX_FILE_BYTES);
    assert!(long.starts_with(&fault.message));
    round_trip(Message::Error {
        code: "worker_error".into(),
        message: "x".repeat(MAX_MESSAGE_BYTES),
    });
}

#[test]
fn a_maximal_snapshot_fits_in_one_frame() {
    let limits = SnapshotLimits::DEFAULT;
    let framing = 1 + 4 + limits.max_files * (8 + 255);
    assert!(limits.max_total_bytes + framing + SAVES_BYTES <= MAX_FRAME);
}

#[test]
fn bad_saves_are_rejected() {
    // Nine entries, a repeated slot, a slot out of range, an oversized
    // slot: each is a protocol error, not a panic.
    let base = encode(&Message::Load {
        snapshot: Snapshot::empty(),
        saves: Vec::new(),
        net: NetEnv::default(),
    });
    // Drop the empty saves count and the net env (1 + 4 bytes).
    let with = |tail: &[u8]| {
        let mut body = base[4..base.len() - 4 - 5].to_vec();
        body.extend_from_slice(tail);
        body.push(0);
        body.extend_from_slice(&0u32.to_le_bytes());
        decode(&body)
    };
    let mut nine = 9u32.to_le_bytes().to_vec();
    for s in 0..9u8 {
        nine.push(s);
        nine.extend_from_slice(&0u32.to_le_bytes());
    }
    assert!(with(&nine).is_err());
    let mut twice = 2u32.to_le_bytes().to_vec();
    for _ in 0..2 {
        twice.push(1);
        twice.extend_from_slice(&0u32.to_le_bytes());
    }
    assert!(with(&twice).is_err());
    let mut high = 1u32.to_le_bytes().to_vec();
    high.push(8);
    high.extend_from_slice(&0u32.to_le_bytes());
    assert!(with(&high).is_err());
    let mut big = 1u32.to_le_bytes().to_vec();
    big.push(0);
    big.extend_from_slice(&(SLOT_BYTES as u32 + 1).to_le_bytes());
    big.extend(vec![0u8; SLOT_BYTES + 1]);
    assert!(with(&big).is_err());
}

#[test]
fn bad_network_fields_are_rejected() {
    let step = |events: &[u8]| {
        let mut body = vec![TAG_STEP, 0];
        body.extend_from_slice(events);
        decode(&body)
    };
    // Too many events.
    assert!(step(&(MAX_BATCH as u32 + 1).to_le_bytes()).is_err());
    // Peer 0.
    let mut ev = 1u32.to_le_bytes().to_vec();
    ev.push(EV_CONNECTED);
    ev.extend_from_slice(&0u32.to_le_bytes());
    assert!(step(&ev).is_err());
    // Empty data.
    let mut ev = 1u32.to_le_bytes().to_vec();
    ev.push(EV_MESSAGE);
    ev.extend_from_slice(&1u32.to_le_bytes());
    ev.extend_from_slice(&0u32.to_le_bytes());
    assert!(step(&ev).is_err());
    // Oversized data is refused from the length alone.
    let mut ev = 1u32.to_le_bytes().to_vec();
    ev.push(EV_MESSAGE);
    ev.extend_from_slice(&1u32.to_le_bytes());
    ev.extend_from_slice(&(MAX_DATA as u32 + 1).to_le_bytes());
    ev.extend(vec![0u8; MAX_DATA + 1]);
    assert!(step(&ev).is_err());
    // An unknown failure code.
    let mut ev = 1u32.to_le_bytes().to_vec();
    ev.push(EV_FAILED);
    ev.extend_from_slice(&5u32.to_le_bytes());
    ev.extend_from_slice(b"net_x");
    ev.extend_from_slice(&0u32.to_le_bytes());
    assert!(step(&ev).is_err());
    // An unknown tag.
    let mut ev = 1u32.to_le_bytes().to_vec();
    ev.push(99);
    assert!(step(&ev).is_err());
    // A frame with too many commands or too much room.
    let base = encode(&Message::Frame {
        frame: OwnedFrame {
            frame: 1,
            width: 1,
            height: 1,
            pixels: vec![0],
            palette: [[0; 3]; PALETTE_SIZE],
            profile: FrameProfile::default(),
            log: vec![],
            audio: vec![],
            state: ConsoleState::Running,
        },
        saves: Vec::new(),
        commands: Vec::new(),
        net_room: 0,
    });
    let body = &base[4..];
    let mut many = body[..body.len() - 8].to_vec();
    many.extend_from_slice(&(MAX_COMMANDS as u32 + 1).to_le_bytes());
    many.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode(&many).is_err());
    let mut room = body[..body.len() - 4].to_vec();
    room.extend_from_slice(&(MAX_BATCH as u32 + 1).to_le_bytes());
    assert!(decode(&room).is_err());
}

#[test]
fn bad_frames_are_rejected() {
    let mut oversized = (MAX_FRAME as u32 + 1).to_le_bytes().to_vec();
    oversized.push(TAG_STOP);
    assert!(matches!(
        read_frame(&mut std::io::Cursor::new(oversized)),
        Err(ReadError::Proto(_))
    ));
    let zero = 0u32.to_le_bytes().to_vec();
    assert!(matches!(
        read_frame(&mut std::io::Cursor::new(zero)),
        Err(ReadError::Proto(_))
    ));
    assert!(decode(&[0x7f]).is_err(), "unknown tag");
    assert!(decode(&[TAG_STOP, 0]).is_err(), "trailing byte");
    assert!(decode(&[TAG_STEP]).is_err(), "short payload");
    assert!(decode(&[TAG_STEP, 0]).is_err(), "a step without its events");
    // A frame claiming a huge size fails before allocating pixels.
    let mut body = vec![TAG_FRAME];
    body.extend_from_slice(&1u64.to_le_bytes());
    body.extend_from_slice(&5000u32.to_le_bytes());
    body.extend_from_slice(&5000u32.to_le_bytes());
    assert!(decode(&body).unwrap_err().message.contains("refused"));
    // A truncated frame body is a pipe error, not a panic.
    let mut cut = encode(&Message::Ready {
        width: 1,
        height: 1,
    });
    cut.truncate(cut.len() - 2);
    assert!(matches!(
        read_frame(&mut std::io::Cursor::new(cut)),
        Err(ReadError::Io(_))
    ));
    assert!(read_frame(&mut std::io::Cursor::new(Vec::new()))
        .unwrap()
        .is_none());
}
