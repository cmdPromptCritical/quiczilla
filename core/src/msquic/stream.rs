use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::ffi::*;

#[repr(C)]
struct SendBuffer {
    quic_buffer: QUIC_BUFFER,
    data: Vec<u8>,
}

pub struct StreamShared {
    pub stream_handle: HQUIC,
    pub recv_buf: Vec<u8>,
    pub recv_waker: Option<Waker>,
    pub send_waker: Option<Waker>,
    pub is_closed_read: bool,
    pub is_closed_write: bool,
    pub in_flight_bytes: usize,
    pub error: Option<io::Error>,
}

pub struct MsQuicSendStream {
    pub shared: Arc<Mutex<StreamShared>>,
}

pub struct MsQuicRecvStream {
    pub shared: Arc<Mutex<StreamShared>>,
}

pub struct MsQuicStream {
    pub send: MsQuicSendStream,
    pub recv: MsQuicRecvStream,
}

impl MsQuicStream {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(stream_handle: HQUIC) -> (MsQuicSendStream, MsQuicRecvStream) {
        let shared = Arc::new(Mutex::new(StreamShared {
            stream_handle,
            recv_buf: Vec::with_capacity(128 * 1024),
            recv_waker: None,
            send_waker: None,
            is_closed_read: false,
            is_closed_write: false,
            in_flight_bytes: 0,
            error: None,
        }));

        let send = MsQuicSendStream {
            shared: shared.clone(),
        };
        let recv = MsQuicRecvStream { shared };

        (send, recv)
    }

    pub fn split(self) -> (MsQuicRecvStream, MsQuicSendStream) {
        (self.recv, self.send)
    }
}

unsafe impl Send for StreamShared {}
unsafe impl Sync for StreamShared {}
unsafe impl Send for MsQuicSendStream {}
unsafe impl Sync for MsQuicSendStream {}
unsafe impl Send for MsQuicRecvStream {}
unsafe impl Sync for MsQuicRecvStream {}
unsafe impl Send for MsQuicStream {}
unsafe impl Sync for MsQuicStream {}

/// Callback invoked by MsQuic when a stream event occurs.
///
/// # Safety
///
/// Must be called by MsQuic runtime with valid context and event pointers conforming to MsQuic ABI.
pub unsafe extern "C" fn msquic_stream_callback(
    _stream: HQUIC,
    context: *mut std::ffi::c_void,
    event: *mut QUIC_STREAM_EVENT,
) -> QUIC_STATUS {
    if context.is_null() || event.is_null() {
        return QUIC_STATUS_SUCCESS;
    }

    let shared_arc = unsafe { &*(context as *const Mutex<StreamShared>) };
    let mut state = shared_arc.lock().unwrap();
    let ev = unsafe { &*event };
    match ev.Type {
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_RECEIVE => {
            let recv_ev = unsafe { &ev.__bindgen_anon_1.RECEIVE };
            let buffer_count = recv_ev.BufferCount as usize;
            let buffers_slice =
                unsafe { std::slice::from_raw_parts(recv_ev.Buffers, buffer_count) };

            for buf in buffers_slice.iter() {
                if buf.Length > 0 && !buf.Buffer.is_null() {
                    let slice =
                        unsafe { std::slice::from_raw_parts(buf.Buffer, buf.Length as usize) };
                    state.recv_buf.extend_from_slice(slice);
                }
            }

            if let Some(waker) = state.recv_waker.take() {
                waker.wake();
            }
        }
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_SEND_COMPLETE => {
            let send_complete = unsafe { &ev.__bindgen_anon_1.SEND_COMPLETE };
            if !send_complete.ClientContext.is_null() {
                // Free the heap allocated chunk
                let ctx = unsafe { Box::from_raw(send_complete.ClientContext as *mut SendBuffer) };
                state.in_flight_bytes = state.in_flight_bytes.saturating_sub(ctx.data.len());
            }
            if let Some(waker) = state.send_waker.take() {
                waker.wake();
            }
        }
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_PEER_SEND_SHUTDOWN => {
            state.is_closed_read = true;
            if let Some(waker) = state.recv_waker.take() {
                waker.wake();
            }
        }
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_PEER_RECEIVE_ABORTED => {
            state.is_closed_write = true;
            state.error = Some(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Peer aborted receive",
            ));
            if let Some(waker) = state.send_waker.take() {
                waker.wake();
            }
        }
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_PEER_SEND_ABORTED => {
            state.is_closed_read = true;
            state.error = Some(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Peer aborted send",
            ));
            if let Some(waker) = state.recv_waker.take() {
                waker.wake();
            }
        }
        QUIC_STREAM_EVENT_TYPE_QUIC_STREAM_EVENT_SHUTDOWN_COMPLETE => {
            state.is_closed_read = true;
            state.is_closed_write = true;
            if let Some(waker) = state.recv_waker.take() {
                waker.wake();
            }
            if let Some(waker) = state.send_waker.take() {
                waker.wake();
            }
        }
        _ => {}
    }

    QUIC_STATUS_SUCCESS
}

impl AsyncRead for MsQuicRecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self.shared.lock().unwrap();

        if let Some(ref err) = state.error {
            return Poll::Ready(Err(io::Error::new(err.kind(), err.to_string())));
        }

        if !state.recv_buf.is_empty() {
            let to_read = std::cmp::min(buf.remaining(), state.recv_buf.len());
            buf.put_slice(&state.recv_buf[..to_read]);
            state.recv_buf.drain(..to_read);
            return Poll::Ready(Ok(()));
        }

        if state.is_closed_read {
            return Poll::Ready(Ok(())); // EOF
        }

        state.recv_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for MsQuicSendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.shared.lock().unwrap();

        if let Some(ref err) = state.error {
            return Poll::Ready(Err(io::Error::new(err.kind(), err.to_string())));
        }

        if state.is_closed_write {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Stream closed for writing",
            )));
        }

        const MAX_IN_FLIGHT_BYTES: usize = 16 * 1024 * 1024;
        if state.in_flight_bytes >= MAX_IN_FLIGHT_BYTES {
            state.send_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let api = match super::ffi::get_api() {
            Ok(a) => a,
            Err(e) => return Poll::Ready(Err(io::Error::other(e.to_string()))),
        };

        let stream_send = match api.StreamSend {
            Some(f) => f,
            None => return Poll::Ready(Err(io::Error::other("StreamSend missing"))),
        };

        // Box a copy of the buffer to remain valid until SEND_COMPLETE
        let mut send_buf = Box::new(SendBuffer {
            quic_buffer: QUIC_BUFFER {
                Length: 0,
                Buffer: std::ptr::null_mut(),
            },
            data: buf.to_vec(),
        });
        send_buf.quic_buffer.Length = send_buf.data.len() as u32;
        send_buf.quic_buffer.Buffer = send_buf.data.as_mut_ptr();
        let buffer_len = send_buf.quic_buffer.Length;
        let raw_box = Box::into_raw(send_buf);

        state.in_flight_bytes += buffer_len as usize;

        let status = unsafe {
            stream_send(
                state.stream_handle,
                &(*raw_box).quic_buffer,
                1,
                QUIC_SEND_FLAGS_QUIC_SEND_FLAG_NONE,
                raw_box as *mut std::ffi::c_void,
            )
        };

        if !super::ffi::is_quic_success_or_pending(status) {
            state.in_flight_bytes = state.in_flight_bytes.saturating_sub(buffer_len as usize);
            // reclaim memory on immediate error
            let _ = unsafe { Box::from_raw(raw_box) };
            return Poll::Ready(Err(io::Error::other(format!(
                "StreamSend failed with status 0x{:08x}",
                status
            ))));
        }

        Poll::Ready(Ok(buffer_len as usize))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.shared.lock().unwrap();
        if state.in_flight_bytes > 0 {
            state.send_waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.shared.lock().unwrap();
        if state.in_flight_bytes > 0 {
            state.send_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        if !state.is_closed_write {
            let api = match super::ffi::get_api() {
                Ok(a) => a,
                Err(e) => return Poll::Ready(Err(io::Error::other(e.to_string()))),
            };

            if let Some(stream_shutdown) = api.StreamShutdown {
                unsafe {
                    stream_shutdown(
                        state.stream_handle,
                        QUIC_STREAM_SHUTDOWN_FLAGS_QUIC_STREAM_SHUTDOWN_FLAG_GRACEFUL,
                        0,
                    );
                }
            }
            state.is_closed_write = true;
        }

        Poll::Ready(Ok(()))
    }
}
