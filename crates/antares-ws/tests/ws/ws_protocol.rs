// SPDX-License-Identifier: EUPL-1.2
//! RFC 6455 at the socket: the close handshake and the codes a broken frame
//! closes with (5.5.1, 7.4.1). The conformance repo's `tests/ws/rfc6455_probe.py`
//! asks the same of a running broker.

use crate::common::{broker, connect, Client};
use antares_ws::Limits;
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{CloseCode, Control, Data, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::{Frame, FrameHeader};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

async fn raw(c: &mut Client, frame: Frame) -> Result<(), String> {
    c.ws_mut()
        .send(Message::Frame(frame))
        .await
        .map_err(|e| e.to_string())
}

/// A client's close 1000 is answered by a close 1000 before the socket ends.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_client_close_is_answered_with_a_close_frame() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let bye = CloseFrame {
        code: CloseCode::Normal,
        reason: "bye".into(),
    };
    c.ws_mut()
        .send(Message::Close(Some(bye)))
        .await
        .map_err(|e| e.to_string())?;
    assert_eq!(c.closed().await?.0, 1000);
    Ok(())
}

/// Reserved bits, a reserved opcode and a fragmented ping close with 1002;
/// a text frame that is not UTF-8 closes with 1007.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_frame_that_breaks_the_protocol_closes_with_its_code() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut rsv = Frame::message(b"{}".to_vec(), OpCode::Data(Data::Text), true);
    rsv.header_mut().rsv1 = true;
    let cases = [
        ("reserved bits", rsv, 1002),
        (
            "reserved opcode",
            Frame::message(Vec::new(), OpCode::Data(Data::Reserved(3)), true),
            1002,
        ),
        (
            "fragmented ping",
            Frame::from_payload(
                FrameHeader {
                    is_final: false,
                    opcode: OpCode::Control(Control::Ping),
                    ..FrameHeader::default()
                },
                b"a".to_vec().into(),
            ),
            1002,
        ),
        (
            "text not UTF-8",
            Frame::message(vec![0xce, 0xba, 0xff], OpCode::Data(Data::Text), true),
            1007,
        ),
    ];
    for (what, frame, want) in cases {
        let mut c = connect(b.addr, ROOT, &[]).await?;
        raw(&mut c, frame).await?;
        assert_eq!(c.closed().await?.0, want, "{what}");
    }
    Ok(())
}
