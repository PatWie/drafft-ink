//! GPU time of the composite pass, from timestamp queries.
//!
//! The pass writes a timestamp at its start and at its end. The two are resolved into a small buffer and copied to a
//! second one that the CPU can map. The map is asked for after the submit and read a frame or more later, so
//! nothing waits for the GPU: while one result is on its way no new measurement starts, and the panel shows the
//! latest finished one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::gpu::Gpu;

/// Two 64-bit timestamps.
const RESULT_BYTES: u64 = 2 * 8;

/// How the map of the readback buffer stands.
const MAP_PENDING: u8 = 0;
const MAP_READY: u8 = 1;
const MAP_FAILED: u8 = 2;

/// What one pass needs to be timed. The handles are clones, so the compositor can keep borrowing itself while it
/// encodes.
pub(crate) struct Measure {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}

impl Measure {
    /// The timestamp writes for the pass: one at its start and one at its end.
    pub(crate) fn writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        }
    }

    /// Resolves the two timestamps and copies them where the CPU can map them. Call it on the encoder of the timed
    /// pass, after the pass.
    pub(crate) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, RESULT_BYTES);
    }
}

/// Timestamps around one pass, read back without waiting.
pub(crate) struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
    /// The map in flight, if any.
    mapping: Option<Arc<AtomicU8>>,
    /// The latest finished measurement, in milliseconds.
    last_ms: Option<f32>,
}

impl GpuTimer {
    /// A timer, or `None` when the device was opened without timestamp queries.
    pub(crate) fn new(gpu: &Gpu) -> Option<Self> {
        if !gpu.device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = gpu.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("composite timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let buffer = |label, usage| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: RESULT_BYTES,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries,
            resolve: buffer("timestamp resolve", wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readback: buffer("timestamp readback", wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
            period_ns: gpu.queue.get_timestamp_period(),
            mapping: None,
            last_ms: None,
        })
    }

    /// The handles for timing the next pass, or `None` while the previous result is still on its way.
    pub(crate) fn begin(&self) -> Option<Measure> {
        self.mapping.is_none().then(|| Measure {
            queries: self.queries.clone(),
            resolve: self.resolve.clone(),
            readback: self.readback.clone(),
        })
    }

    /// Asks for the map of the result. Call it after the submit that holds the encoder of [`Self::resolve`].
    pub(crate) fn submitted(&mut self) {
        let state = Arc::new(AtomicU8::new(MAP_PENDING));
        let flag = state.clone();
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            flag.store(if result.is_ok() { MAP_READY } else { MAP_FAILED }, Ordering::Release);
        });
        self.mapping = Some(state);
    }

    /// Drives the map without waiting and takes a finished result. Returns the latest measurement.
    pub(crate) fn collect(&mut self, gpu: &Gpu) -> Option<f32> {
        if let Some(state) = self.mapping.clone() {
            // The poll lets wgpu run the map callback of work that has finished. It never blocks. On the web the
            // browser runs the callback from its own event loop.
            #[cfg(not(target_arch = "wasm32"))]
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            #[cfg(target_arch = "wasm32")]
            let _ = gpu;
            match state.load(Ordering::Acquire) {
                MAP_READY => {
                    self.last_ms = self.read();
                    self.readback.unmap();
                    self.mapping = None;
                }
                MAP_FAILED => self.mapping = None,
                _ => {}
            }
        }
        self.last_ms
    }

    /// Reads the two timestamps from the mapped buffer.
    fn read(&self) -> Option<f32> {
        let mapped = self.readback.slice(..).get_mapped_range().ok()?;
        let (start, end) = ticks(&mapped)?;
        Some(ticks_to_ms(start, end, self.period_ns))
    }
}

/// The start and end ticks in the 16 bytes the GPU wrote, or `None` when the bytes are too few.
fn ticks(bytes: &[u8]) -> Option<(u64, u64)> {
    let word = |at: usize| -> Option<u64> {
        let raw: [u8; 8] = bytes.get(at..at + 8)?.try_into().ok()?;
        Some(u64::from_ne_bytes(raw))
    };
    Some((word(0)?, word(8)?))
}

/// Milliseconds between two timestamps. A counter that went backwards (a wrapped or reset clock) gives 0.
fn ticks_to_ms(start: u64, end: u64, period_ns: f32) -> f32 {
    (end.saturating_sub(start) as f64 * f64::from(period_ns) / 1e6) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_read_in_order_and_short_buffers_give_nothing() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&100u64.to_ne_bytes());
        bytes.extend_from_slice(&2_600u64.to_ne_bytes());
        assert_eq!(ticks(&bytes), Some((100, 2_600)));
        assert_eq!(ticks(&bytes[..15]), None);
    }

    #[test]
    fn ticks_become_milliseconds_with_the_queue_period() {
        // 2,500 ticks of 1,000 ns are 2.5 ms.
        assert_eq!(ticks_to_ms(100, 2_600, 1_000.0), 2.5);
        assert_eq!(ticks_to_ms(500, 100, 1.0), 0.0, "a clock that went back is not a negative time");
    }
}
