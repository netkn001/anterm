use super::tokenizer::MAX_APC_PAYLOAD;
use super::*;

fn collect(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut tok = ApcTokenizer::new();
    let mut out = Vec::new();
    for c in chunks {
        tok.feed(c, |_| {}, |cmd| out.push(cmd.to_vec()));
    }
    out
}

/// Feed `chunks` and return the concatenated passthrough stream (input with
/// every `_G` sequence stripped) alongside the extracted commands.
fn split(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut tok = ApcTokenizer::new();
    let mut pass = Vec::new();
    let mut cmds = Vec::new();
    for c in chunks {
        tok.feed(
            c,
            |b| pass.extend_from_slice(b),
            |cmd| cmds.push(cmd.to_vec()),
        );
    }
    (pass, cmds)
}

#[test]
fn extracts_a_graphics_command_between_esc_underscore_and_st() {
    assert_eq!(
        collect(&[b"\x1b_Ga=T,f=32;AAAA\x1b\\"]),
        vec![b"Ga=T,f=32;AAAA".to_vec()]
    );
}

#[test]
fn non_graphics_apc_is_ignored() {
    // An APC that doesn't start with `G` (e.g. some other program's APC) is
    // discarded, and a graphics command right after is still caught.
    assert_eq!(
        collect(&[b"\x1b_Xhello\x1b\\\x1b_Ga=q;AAAA\x1b\\"]),
        vec![b"Ga=q;AAAA".to_vec()]
    );
}

#[test]
fn command_split_across_reads_is_reassembled() {
    assert_eq!(
        collect(&[b"\x1b_Ga=T,", b"f=32;AA", b"AA\x1b", b"\\"]),
        vec![b"Ga=T,f=32;AAAA".to_vec()]
    );
}

#[test]
fn byte_at_a_time_delivery_crosses_every_state() {
    let stream = b"plain\x1b_Ga=q;AAAA\x1b\\more";
    let chunks: Vec<&[u8]> = stream.chunks(1).collect();
    assert_eq!(collect(&chunks), vec![b"Ga=q;AAAA".to_vec()]);
}

#[test]
fn passthrough_is_input_with_graphics_stripped() {
    let (pass, cmds) = split(&[b"before\x1b_Ga=T;AAAA\x1b\\after"]);
    assert_eq!(pass, b"beforeafter".to_vec());
    assert_eq!(cmds, vec![b"Ga=T;AAAA".to_vec()]);
}

#[test]
fn non_graphics_apc_passes_through_verbatim() {
    // A foreign APC (e.g. tmux) must reach the client's VT parser unchanged,
    // while a graphics command in the same stream is still stripped.
    let (pass, cmds) = split(&[b"\x1b_Xhello\x1b\\\x1b_Ga=q;AAAA\x1b\\end"]);
    assert_eq!(pass, b"\x1b_Xhello\x1b\\end".to_vec());
    assert_eq!(cmds, vec![b"Ga=q;AAAA".to_vec()]);
}

#[test]
fn lone_esc_and_other_escapes_are_preserved_in_passthrough() {
    // A CSI sequence and a bare ESC must survive; only `_G` is removed.
    let (pass, cmds) = split(&[b"a\x1b[31mred\x1b_Ga=d\x1b\\z"]);
    assert_eq!(pass, b"a\x1b[31mredz".to_vec());
    assert_eq!(cmds, vec![b"Ga=d".to_vec()]);
}

#[test]
fn passthrough_survives_being_split_mid_sequence() {
    // Feed the same stream one byte at a time; the passthrough must still be
    // exactly the input minus the graphics command.
    let stream = b"x\x1b_Ga=T;QUJD\x1b\\y\x1b_Zother\x1b\\z";
    let chunks: Vec<&[u8]> = stream.chunks(1).collect();
    let (pass, cmds) = split(&chunks);
    assert_eq!(pass, b"xy\x1b_Zother\x1b\\z".to_vec());
    assert_eq!(cmds, vec![b"Ga=T;QUJD".to_vec()]);
}

#[test]
fn sniffer_ties_tokenizer_and_parser_together() {
    let mut s = GraphicsSniffer::new();
    let mut pass = Vec::new();
    let events = s.feed(b"hi\x1b_Gi=1,a=q,t=d;AAAA\x1b\\bye", |b| {
        pass.extend_from_slice(b)
    });
    assert_eq!(pass, b"hibye".to_vec());
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Query { reply, honored } => {
            assert!(honored);
            assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
        }
        _ => panic!("expected query"),
    }
}

#[test]
fn sniff_takes_zero_copy_fast_path_without_graphics() {
    let mut s = GraphicsSniffer::new();
    // A chunk full of CSI escapes but no APC borrows straight through.
    match s.sniff(b"\x1b[31mred\x1b[0m plain") {
        Sniffed::Plain(b) => assert_eq!(b, b"\x1b[31mred\x1b[0m plain"),
        Sniffed::Segments(_) => panic!("expected the borrowed fast path"),
    }
}

#[test]
fn sniff_preserves_stream_order_of_text_and_images() {
    let pixel = [0xffu8, 0x00, 0x00, 0xff];
    let b64 = BASE64.encode(pixel);
    let stream = format!("A\x1b_Ga=T,f=32,t=d,s=1,v=1,i=1;{b64}\x1b\\B");
    let mut s = GraphicsSniffer::new();
    let segs = match s.sniff(stream.as_bytes()) {
        Sniffed::Segments(segs) => segs,
        Sniffed::Plain(_) => panic!("graphics present"),
    };
    // Text "A", then the image, then text "B" — in that order.
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0], Segment::Output(b"A".to_vec()));
    assert!(matches!(&segs[1], Segment::Image(img) if img.id == 1));
    assert_eq!(segs[2], Segment::Output(b"B".to_vec()));
}

#[test]
fn sniff_emits_query_and_delete_segments() {
    let mut s = GraphicsSniffer::new();
    let segs = match s.sniff(b"\x1b_Gi=7,a=q,t=d;AAAA\x1b\\x\x1b_Ga=d,d=A\x1b\\") {
        Sniffed::Segments(segs) => segs,
        Sniffed::Plain(_) => panic!("graphics present"),
    };
    assert_eq!(segs[0], Segment::Query(b"\x1b_Gi=7;OK\x1b\\".to_vec()));
    assert_eq!(segs[1], Segment::Output(b"x".to_vec()));
    assert!(matches!(&segs[2], Segment::Delete(d) if d.target == b'A'));
}

#[test]
fn sniff_coalesces_adjacent_passthrough_runs() {
    // A foreign APC between two text runs is passthrough, so the whole thing
    // collapses to a single output segment (no graphics segment at all).
    let mut s = GraphicsSniffer::new();
    match s.sniff(b"a\x1b_Zother\x1b\\b") {
        Sniffed::Segments(segs) => {
            assert_eq!(segs, vec![Segment::Output(b"a\x1b_Zother\x1b\\b".to_vec())]);
        }
        Sniffed::Plain(_) => panic!("an APC opener forces the slow path"),
    }
}

#[test]
fn resyncs_on_new_apc_after_an_unterminated_one() {
    assert_eq!(
        collect(&[b"\x1b_Ga=T;dropped\x1b_Ga=q;AAAA\x1b\\"]),
        vec![b"Ga=q;AAAA".to_vec()]
    );
}

#[test]
fn oversized_chunk_is_abandoned_and_stream_recovers() {
    let mut big = b"\x1b_G".to_vec();
    big.extend(std::iter::repeat_n(b'x', MAX_APC_PAYLOAD + 1));
    big.extend_from_slice(b"\x1b\\\x1b_Ga=q;AAAA\x1b\\");
    assert_eq!(collect(&[&big]), vec![b"Ga=q;AAAA".to_vec()]);
}

#[test]
fn control_parses_the_keys_terminal_browser_sends() {
    let c =
        Control::parse(b"Ga=T,f=32,o=z,s=1920,v=1080,t=d,i=42,p=1,C=1,q=2,m=1;xxxx").unwrap();
    assert_eq!(c.action, Action::TransmitAndDisplay);
    assert_eq!(c.format, WireFormat::Rgba);
    assert!(c.compressed);
    assert_eq!(c.width, 1920);
    assert_eq!(c.height, 1080);
    assert_eq!(c.medium, Medium::Direct);
    assert_eq!(c.id, 42);
    assert!(c.more);
    assert_eq!(c.quiet, 2);
}

#[test]
fn query_reply_refuses_shm_file_on_remote_pane() {
    // The default parser is non-local (the SSH-safe posture): direct is
    // honored, file/shm are refused so the sender falls back to inline.
    let mut p = GraphicsParser::new();
    // The exact probe terminal-browser's graphics.ts sends.
    let ev = p.feed(b"Gi=4207,a=q,t=d,f=24,s=1,v=1;AAAA").unwrap();
    match ev {
        Event::Query { reply, honored } => {
            assert!(honored);
            assert_eq!(reply, b"\x1b_Gi=4207;OK\x1b\\".to_vec());
        }
        _ => panic!("expected query"),
    }
    // The shm/file medium probes must be refused so the sender falls back.
    let ev = p.feed(b"Gi=299,a=q,t=s,f=32,s=1,v=1;L3B4LXE").unwrap();
    match ev {
        Event::Query { reply, honored } => {
            assert!(!honored);
            assert_eq!(reply, b"\x1b_Gi=299;ENOTSUPPORTED\x1b\\".to_vec());
        }
        _ => panic!("expected query"),
    }
}

#[test]
#[cfg(unix)]
fn query_reply_ok_for_shm_file_on_local_pane() {
    // A local pane shares the sender's filesystem, so file/shm are honored:
    // this is what unlocks terminal-browser's zero-inflate fast path.
    let mut p = GraphicsParser::new_local(true);
    for probe in [
        &b"Gi=299,a=q,t=s,f=32,s=1,v=1;L3B4LXE"[..],
        &b"Gi=300,a=q,t=f,f=32,s=1,v=1;L3RtcC94"[..],
        &b"Gi=301,a=q,t=t,f=32,s=1,v=1;L3RtcC94"[..],
    ] {
        match p.feed(probe).unwrap() {
            Event::Query { honored, reply } => {
                assert!(honored, "local pane should honor {probe:?}");
                assert!(reply.ends_with(b";OK\x1b\\"), "reply was {reply:?}");
            }
            _ => panic!("expected query"),
        }
    }
}

#[test]
#[cfg(unix)]
fn shared_transmission_surfaces_medium_transfer_on_local() {
    // `terminal-browser`'s shm transmit template: raw f=32, no o=z, the
    // shm object name base64'd after the `;`.
    let name = b"/px-abc123";
    let b64 = BASE64.encode(name);
    let cmd = format!("Ga=T,f=32,t=s,s=64,v=1,i=7,S=256;{b64}");
    let mut p = GraphicsParser::new_local(true);
    match p.feed(cmd.as_bytes()).unwrap() {
        Event::ImageFromMedium(t) => {
            assert_eq!(t.medium, Medium::Shared);
            assert_eq!(t.name, name);
            assert_eq!(t.id, 7);
            assert_eq!((t.width, t.height), (64, 1));
            assert_eq!(t.size, 256);
            assert!(!t.compressed);
        }
        _ => panic!("expected medium transfer"),
    }
}

#[test]
fn shared_transmission_dropped_on_remote() {
    // A non-local parser must never surface a file/shm transfer even if a
    // misbehaving sender ignored the refusal and sent one anyway.
    let b64 = BASE64.encode(b"/px-abc123");
    let cmd = format!("Ga=T,f=32,t=s,s=64,v=1,i=7;{b64}");
    let mut p = GraphicsParser::new();
    assert_eq!(p.feed(cmd.as_bytes()), None);
}

#[test]
#[cfg(unix)]
fn file_transfer_resolves_and_temp_file_is_removed() {
    // A temp-file transfer: write raw RGBA to a temp path, resolve it, and
    // confirm the file is deleted afterward (the one-shot handoff contract).
    let rgba = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let dir = std::env::temp_dir();
    let path = dir.join(format!("anterm-kitty-test-{}.rgba", std::process::id()));
    std::fs::write(&path, rgba).unwrap();
    let t = MediumTransfer {
        medium: Medium::TempFile,
        name: path.clone().into_os_string().into_encoded_bytes(),
        offset: 0,
        size: 0,
        id: 1,
        number: 0,
        placement: 0,
        width: 2,
        height: 1,
        cols: 0,
        rows: 0,
        format: WireFormat::Rgba,
        compressed: false,
    };
    let img = t.resolve().expect("resolve temp file");
    assert_eq!(img.data, rgba);
    assert_eq!(img.to_rgba8().unwrap(), rgba);
    assert!(!path.exists(), "temp file should be removed after read");
}

/// Build a `t=t` transfer naming `path`, the shape a hostile escape uses.
#[cfg(unix)]
fn temp_file_transfer(path: &std::path::Path) -> MediumTransfer {
    MediumTransfer {
        medium: Medium::TempFile,
        name: path.to_path_buf().into_os_string().into_encoded_bytes(),
        offset: 0,
        size: 0,
        id: 1,
        number: 0,
        placement: 0,
        width: 2,
        height: 1,
        cols: 0,
        rows: 0,
        format: WireFormat::Rgba,
        compressed: false,
    }
}

#[test]
#[cfg(unix)]
fn a_temp_file_transfer_outside_the_temp_dir_is_read_but_not_deleted() {
    // `t=t` names an arbitrary path out of an escape sequence — `cat`ing a
    // hostile file is enough to send one. Unlinking whatever it points at
    // would delete e.g. `~/.ssh/id_ed25519`. Read it, leave it.
    //
    // `/etc/hosts` stands in for the victim: a real, readable file that is
    // never under a temp dir on any host we build for. Deliberately *not*
    // something derived from `CARGO_MANIFEST_DIR` — a checkout can itself
    // live under `/tmp`, which would make the test assert the opposite of
    // what it means to.
    let victim = std::path::Path::new("/etc/hosts");
    let img = temp_file_transfer(victim).resolve().expect("still reads");
    assert!(!img.data.is_empty());
    assert!(
        victim.exists(),
        "a path outside the temp dir must survive a t=t handoff"
    );
}

#[test]
#[cfg(unix)]
fn the_temp_dir_check_resolves_symlinks_and_dotdot_before_comparing() {
    // The prefix has to be checked on the *canonicalized* path: `/tmp/../etc`
    // starts with `/tmp` textually but lands nowhere near it. macOS also
    // routes both `/tmp` and `TMPDIR` through `/private`, so a raw string
    // compare would reject the legitimate case too.
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("frame.rgba");
    std::fs::write(&inside, [0u8; 4]).unwrap();
    assert!(path_is_in_temp_dir(&inside));

    assert!(!path_is_in_temp_dir(std::path::Path::new(
        "/tmp/../etc/hosts"
    )));
    assert!(!path_is_in_temp_dir(std::path::Path::new("/etc/hosts")));
    // A path that doesn't resolve at all can't be vouched for.
    assert!(!path_is_in_temp_dir(std::path::Path::new(
        "/nonexistent-anterm-test-path"
    )));
    // The temp dir itself is not a file we'd ever unlink.
    assert!(!path_is_in_temp_dir(&std::env::temp_dir()));
}

#[test]
#[cfg(unix)]
fn a_transfer_naming_a_character_device_is_refused() {
    // `fs::read` on `/dev/zero` never returns, and this runs on the daemon's
    // reader thread — the pane's whole output path would wedge.
    let t = temp_file_transfer(std::path::Path::new("/dev/zero"));
    assert_eq!(t.resolve(), None);
}

#[test]
#[cfg(unix)]
fn a_malformed_shm_name_is_refused_before_shm_open() {
    for name in [
        &b"/../../etc/passwd"[..],
        &b"/sub/dir"[..],
        &b"no-leading-slash"[..],
        &b"/"[..],
        &b"/.."[..],
    ] {
        assert!(
            !shm_name_is_wellformed(name),
            "{:?} should be refused",
            String::from_utf8_lossy(name)
        );
    }
    assert!(shm_name_is_wellformed(b"/px-abc123"));
}

#[test]
fn a_bomb_that_declares_huge_dimensions_is_still_bounded() {
    // The declared size is the sender's to choose, so bounding the inflate
    // by it alone isn't enough: `s=65535,v=65535` works out to a 17 GB
    // budget, which hands a bomb back exactly the allocation the bound was
    // meant to deny. The absolute `MAX_IMAGE_BYTES` clamp is what closes it.
    // (`to_rgba8_bounds_the_inflate_by_declared_dimensions` covers the
    // ordinary case, where the declared size is the tighter of the two.)
    let img = Image {
        id: 1,
        number: 0,
        placement: 0,
        width: 65535,
        height: 65535,
        cols: 0,
        rows: 0,
        // Has to inflate past `MAX_IMAGE_BYTES` to exercise the clamp at
        // all — anything smaller decodes on its merits and proves nothing.
        // Zeros deflate to a few KB, so the payload on the wire stays tiny,
        // which is the whole point of the attack.
        data: miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; MAX_IMAGE_BYTES + 1], 6),
        format: WireFormat::Rgba,
        compressed: true,
    };
    assert!(
        img.decoded_len().unwrap() > MAX_IMAGE_BYTES,
        "the declared budget must be the looser bound for this to test anything"
    );
    assert!(
        img.data.len() < 1 << 20,
        "the compressed payload must stay small: {}",
        img.data.len()
    );
    assert_eq!(img.to_rgba8(), None, "the clamp has to reject it");
}

#[test]
fn a_transmission_cap_leaves_room_for_the_wire_frame() {
    // `MAX_TRANSMISSION_BASE64` has to decode to something that still fits
    // in a `MAX_FRAME` wire frame with the header on it — otherwise
    // `write_frame` fails, and the daemon's writer treats that as fatal and
    // drops the client's whole connection over one image.
    let decoded_max = MAX_TRANSMISSION_BASE64 / 4 * 3;
    assert!(
        decoded_max + HEADER_LEN <= crate::daemon::protocol::MAX_FRAME,
        "{decoded_max} + {HEADER_LEN} must fit in {}",
        crate::daemon::protocol::MAX_FRAME
    );
    assert!(MAX_IMAGE_BYTES + HEADER_LEN <= crate::daemon::protocol::MAX_FRAME);
}

#[test]
fn a_quiet_sender_gets_no_reply_written_back_to_its_pty() {
    // The reply is written to the *PTY* — it arrives as if typed. `q=1`
    // suppresses success, `q=2` suppresses failures too.
    let mut p = GraphicsParser::new_local(true);
    assert_eq!(p.feed(b"Gi=1,a=q,t=d,q=1;AAAA"), None);
    assert_eq!(p.feed(b"Gi=2,a=q,t=d,q=2;AAAA"), None);
    // A refusal still reaches a `q=1` sender, which needs to hear it to fall
    // back, and an unquiet probe is answered as before.
    let mut remote = GraphicsParser::new();
    assert!(matches!(
        remote.feed(b"Gi=3,a=q,t=s,q=1;AAAA"),
        Some(Event::Query { honored: false, .. })
    ));
    assert!(matches!(
        p.feed(b"Gi=4,a=q,t=d;AAAA"),
        Some(Event::Query { honored: true, .. })
    ));
}

#[test]
fn a_quiet_key_past_a_byte_does_not_wrap_to_chatty() {
    assert_eq!(Control::parse(b"Ga=q,q=256;").unwrap().quiet, 255);
}

#[test]
fn graphics_after_an_unterminated_foreign_apc_are_still_lifted() {
    // A foreign APC that never sends its ST used to swallow every `ESC _G`
    // after it: the graphics were forwarded as APC text, which the client's
    // vte then discards, so the image vanished with no trace.
    let input = b"\x1b_somevendor\x1b_Ga=q;AAAA\x1b\\";
    assert_eq!(collect(&[&input[..]]), vec![b"Ga=q;AAAA".to_vec()]);
}

#[test]
fn an_escape_interrupting_a_graphics_command_still_reaches_the_client() {
    // The abandoned command's bytes stay stripped, but the escape that
    // interrupted it belongs to the terminal — swallowing it turned a
    // following `\x1b[31m` into literal `31m` on screen.
    let mut t = ApcTokenizer::new();
    let mut out = Vec::new();
    t.feed(b"\x1b_Gxx\x1b[31mred", |b| out.extend_from_slice(b), |_| {});
    assert_eq!(out, b"\x1b[31mred");
}

#[test]
fn a_place_only_command_emits_nothing_rather_than_an_empty_image() {
    // `a=p` places an image transmitted earlier, which needs a stored-image
    // table this parser doesn't keep. It used to fall through the chunk
    // accumulator and emit an `Image` with no payload.
    let mut p = GraphicsParser::new_local(true);
    assert_eq!(p.feed(b"Ga=p,i=42,p=1;"), None);
}

#[test]
#[cfg(unix)]
fn an_oversized_size_key_truncates_instead_of_dropping_the_frame() {
    // kitty truncates an `S=` that runs past the object; discarding the
    // whole frame loses an image a real sender would have seen rendered.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.rgba");
    std::fs::write(&path, [1u8, 2, 3, 4]).unwrap();
    let mut t = temp_file_transfer(&path);
    t.medium = Medium::File; // leave the file in place
    t.size = 4096;
    assert_eq!(t.resolve().expect("truncated, not dropped").data.len(), 4);
}

#[test]
fn single_shot_rgba_transmission_decodes() {
    // One 1x1 opaque-red RGBA pixel, uncompressed, direct.
    let pixel = [0xffu8, 0x00, 0x00, 0xff];
    let b64 = BASE64.encode(pixel);
    let cmd = format!("Ga=T,f=32,t=d,s=1,v=1,i=7;{b64}");
    let mut p = GraphicsParser::new();
    let ev = p.feed(cmd.as_bytes()).unwrap();
    match ev {
        Event::Image(img) => {
            assert_eq!(img.id, 7);
            assert_eq!((img.width, img.height), (1, 1));
            assert!(!img.compressed);
            assert_eq!(img.to_rgba8().unwrap(), pixel);
            assert_eq!(img.format, WireFormat::Rgba);
        }
        _ => panic!("expected image"),
    }
}

#[test]
fn chunked_compressed_transmission_reassembles_and_inflates() {
    // 2x1 RGBA (red, green), zlib-compressed, split into two direct chunks
    // exactly the way terminal-browser frames a `t=d,o=z` transmission.
    let rgba = [0xffu8, 0, 0, 0xff, 0x00, 0xff, 0x00, 0xff];
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&rgba, 1);
    let b64 = BASE64.encode(&z);
    let mid = b64.len() / 2;
    let first = format!("Ga=T,f=32,o=z,t=d,s=2,v=1,i=9,m=1;{}", &b64[..mid]);
    let second = format!("Gm=0;{}", &b64[mid..]);

    let mut p = GraphicsParser::new();
    assert_eq!(p.feed(first.as_bytes()), None); // more chunks pending
    let ev = p.feed(second.as_bytes()).unwrap();
    match ev {
        Event::Image(img) => {
            assert_eq!(img.id, 9);
            assert_eq!((img.width, img.height), (2, 1));
            assert!(img.compressed);
            // Wire payload stays compressed; the client inflates.
            assert_eq!(img.data, z);
            assert_eq!(img.to_rgba8().unwrap(), rgba);
        }
        _ => panic!("expected image"),
    }
}

#[test]
fn rgb_transmission_is_expanded_to_opaque_rgba() {
    let rgb = [0x10u8, 0x20, 0x30]; // one pixel
    let b64 = BASE64.encode(rgb);
    let cmd = format!("Ga=T,f=24,t=d,s=1,v=1;{b64}");
    let mut p = GraphicsParser::new();
    match p.feed(cmd.as_bytes()).unwrap() {
        Event::Image(img) => {
            assert_eq!(img.to_rgba8().unwrap(), [0x10, 0x20, 0x30, 0xff])
        }
        _ => panic!("expected image"),
    }
}

#[test]
fn delete_is_surfaced() {
    let mut p = GraphicsParser::new();
    match p.feed(b"Ga=d,d=A").unwrap() {
        Event::Delete(c) => {
            assert_eq!(c.action, Action::Delete);
            assert_eq!(c.delete, b'A');
        }
        _ => panic!("expected delete"),
    }
}

#[test]
fn image_delete_normalizes_and_roundtrips() {
    // A bare `a=d` (no selector) means "all visible placements" (`a`).
    let bare = ImageDelete::from_control(&Control::parse(b"Ga=d").unwrap());
    assert_eq!(bare.target, b'a');
    // A scoped delete keeps its selector and id.
    let scoped = ImageDelete::from_control(&Control::parse(b"Ga=d,d=i,i=5").unwrap());
    assert_eq!((scoped.target, scoped.id), (b'i', 5));
    assert_eq!(ImageDelete::decode(&scoped.encode()), Some(scoped));
    assert_eq!(ImageDelete::decode(&[b'a', 0, 0]), None); // too short
}

#[test]
fn image_frame_roundtrips_without_touching_the_payload() {
    let img = Image {
        id: 42,
        number: 3,
        placement: 1,
        width: 1920,
        height: 1080,
        cols: 80,
        rows: 24,
        data: vec![1, 2, 3, 4, 5, 6, 7],
        format: WireFormat::Rgba,
        compressed: true,
    };
    let frame = img.encode_frame();
    // Header + payload, payload byte-identical (no base64 inflation).
    assert_eq!(&frame[HEADER_LEN..], &img.data[..]);
    assert_eq!(Image::decode_frame(&frame), Some(img));
    // A truncated frame is rejected, not panicked on.
    assert_eq!(Image::decode_frame(&frame[..HEADER_LEN - 1]), None);
}

#[test]
fn decode_frame_owned_matches_decode_frame() {
    // The consuming variant reuses the frame's allocation for the pixel
    // buffer; it must reconstruct the exact same `Image` as the borrowing one.
    let img = Image {
        id: 42,
        number: 3,
        placement: 1,
        width: 1920,
        height: 1080,
        cols: 80,
        rows: 24,
        data: vec![9, 8, 7, 6, 5, 4, 3, 2, 1],
        format: WireFormat::Rgb,
        compressed: true,
    };
    let frame = img.encode_frame();
    assert_eq!(Image::decode_frame_owned(frame.clone()), Some(img));
    // Same rejection of a short buffer, no panic on the in-place drain.
    assert_eq!(
        Image::decode_frame_owned(frame[..HEADER_LEN - 1].to_vec()),
        None
    );
}

#[test]
fn take_rgba8_moves_the_uncompressed_buffer_and_matches_to_rgba8() {
    // Uncompressed RGBA: `take_rgba8` returns the exact bytes `to_rgba8`
    // clones, but empties `data` (the buffer was moved, not copied).
    let pixels = vec![0x10u8, 0x20, 0x30, 0x40];
    let mut img = Image {
        id: 0,
        number: 0,
        placement: 0,
        width: 1,
        height: 1,
        cols: 0,
        rows: 0,
        data: pixels.clone(),
        format: WireFormat::Rgba,
        compressed: false,
    };
    let borrowed = img.to_rgba8().unwrap();
    assert_eq!(borrowed, pixels);
    let moved = img.take_rgba8().unwrap();
    assert_eq!(moved, pixels);
    assert!(
        img.data.is_empty(),
        "the pixel buffer was moved out, not cloned"
    );
}

#[test]
fn take_rgba8_still_expands_rgb_and_bounds_the_bomb() {
    // f=24 repacks to RGBA (length changes, so it can't move) — same result
    // as `to_rgba8`.
    let mut rgb = Image {
        id: 0,
        number: 0,
        placement: 0,
        width: 1,
        height: 1,
        cols: 0,
        rows: 0,
        data: vec![0x10, 0x20, 0x30],
        format: WireFormat::Rgb,
        compressed: false,
    };
    assert_eq!(rgb.take_rgba8().unwrap(), [0x10, 0x20, 0x30, 0xff]);

    // The declared-dimension inflate bound is preserved on the moving path.
    let bomb = vec![0u8; 4 * 1024 * 1024];
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&bomb, 9);
    let mut img = Image {
        id: 1,
        number: 0,
        placement: 0,
        width: 1,
        height: 1,
        cols: 0,
        rows: 0,
        data: z,
        format: WireFormat::Rgba,
        compressed: true,
    };
    assert_eq!(
        img.take_rgba8(),
        None,
        "an over-budget inflate is still dropped"
    );
}

#[test]
fn to_rgba8_bounds_the_inflate_by_declared_dimensions() {
    // A hostile payload: a tiny compressed blob that inflates far past the
    // 1x1 image it claims to be. Without a cap this would balloon into a
    // multi-MB (in the wild, multi-GB) allocation; bounded by the declared
    // `width * height * 4` it must be rejected instead.
    let bomb = vec![0u8; 4 * 1024 * 1024]; // 4 MiB of zeros → tiny deflate
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&bomb, 9);
    assert!(z.len() < bomb.len(), "payload must actually compress");
    let img = Image {
        id: 1,
        number: 0,
        placement: 0,
        width: 1,
        height: 1,
        cols: 0,
        rows: 0,
        data: z,
        format: WireFormat::Rgba,
        compressed: true,
    };
    assert_eq!(img.to_rgba8(), None, "an over-budget inflate is dropped");

    // A payload that inflates to exactly its declared size still decodes.
    let pixels = vec![0xabu8; 4]; // 1x1 RGBA
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&pixels, 9);
    let ok = Image { data: z, ..img };
    assert_eq!(ok.to_rgba8().as_deref(), Some(&pixels[..]));
}
