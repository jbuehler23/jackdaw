//! Receives live frame-view pixels from the focused game instance and keeps
//! them in an editor-side image asset for the viewport background.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::time::Instant;

/// Latest streamed frame from the focused instance.
#[derive(Resource, Default)]
pub struct LiveFrameStream {
    /// Image asset holding the newest frame; created on first frame.
    pub image: Option<Handle<Image>>,
    pub size: UVec2,
    pub last_seq: u64,
    /// When the newest frame arrived; `None` until the first frame.
    pub received_at: Option<Instant>,
}

impl LiveFrameStream {
    /// True when a frame arrived recently enough to treat the stream as live.
    pub fn is_fresh(&self) -> bool {
        self.received_at
            .is_some_and(|at| at.elapsed().as_secs_f32() < 1.0)
    }

    /// Drop all stream state (focus change, play stop).
    pub fn clear(&mut self, images: &mut Assets<Image>) {
        if let Some(handle) = self.image.take() {
            images.remove(&handle);
        }
        *self = Self::default();
    }
}

/// Clear the stream from world-level paths (focus change, play stop). A no-op
/// when the stream or image assets are absent, as in headless test worlds.
pub fn clear_stream(world: &mut World) {
    world.try_resource_scope(|world, mut stream: Mut<LiveFrameStream>| {
        if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
            stream.clear(&mut images);
        }
    });
}

/// Apply one decoded frame: (re)create the image asset on size change, else
/// overwrite its pixel data in place. Duplicate frames (same seq) are dropped.
pub fn apply_frame(
    stream: &mut LiveFrameStream,
    images: &mut Assets<Image>,
    frame: jackdaw_pie_protocol::FrameRef<'_>,
) {
    // A zero-size extent would fail render-world validation; the game never sends one.
    if frame.width == 0 || frame.height == 0 {
        return;
    }
    if stream.image.is_some() && frame.seq == stream.last_seq {
        return;
    }
    // The lane is ordered, so a lower seq means the game restarted; treat the
    // frame as the start of a new stream rather than a stale duplicate.
    stream.last_seq = frame.seq;
    stream.received_at = Some(Instant::now());
    let size = UVec2::new(frame.width, frame.height);
    if let Some(handle) = &stream.image
        && stream.size == size
    {
        if let Some(image) = images.get_mut(handle) {
            image.data = Some(frame.pixels.to_vec());
        }
    } else {
        let image = Image::new(
            Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            frame.pixels.to_vec(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        if let Some(old) = stream.image.take() {
            images.remove(&old);
        }
        stream.image = Some(images.add(image));
        stream.size = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_bytes(w: u32, h: u32, seq: u64, fill: u8) -> Vec<u8> {
        jackdaw_pie_protocol::encode_frame(w, h, seq, &vec![fill; (w * h * 4) as usize])
    }

    #[test]
    fn first_frame_creates_image_and_later_frames_update_in_place() {
        let mut images = Assets::<Image>::default();
        let mut stream = LiveFrameStream::default();

        let b1 = frame_bytes(64, 64, 1, 1);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b1).unwrap(),
        );
        let handle = stream.image.clone().expect("image created");
        assert_eq!(stream.size, UVec2::new(64, 64));

        let b2 = frame_bytes(64, 64, 2, 9);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b2).unwrap(),
        );
        assert_eq!(stream.image.as_ref(), Some(&handle), "same asset reused");
        let data = images.get(&handle).unwrap().data.as_ref().unwrap();
        assert!(data.iter().all(|&b| b == 9));
    }

    #[test]
    fn duplicate_seq_is_dropped_and_regression_restarts() {
        let mut images = Assets::<Image>::default();
        let mut stream = LiveFrameStream::default();
        let b2 = frame_bytes(64, 64, 2, 2);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b2).unwrap(),
        );
        let handle = stream.image.clone().expect("image created");

        let dup = frame_bytes(64, 64, 2, 5);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&dup).unwrap(),
        );
        let data = images.get(&handle).unwrap().data.as_ref().unwrap();
        assert!(data.iter().all(|&b| b == 2), "duplicate seq dropped");

        let b1 = frame_bytes(64, 64, 1, 1);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b1).unwrap(),
        );
        assert_eq!(stream.last_seq, 1, "seq regression starts a new stream");
        let data = images.get(&handle).unwrap().data.as_ref().unwrap();
        assert!(data.iter().all(|&b| b == 1), "restart frame applied");

        let b3 = frame_bytes(128, 64, 3, 3);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b3).unwrap(),
        );
        assert_eq!(stream.size, UVec2::new(128, 64));
    }

    #[test]
    fn zero_size_frame_is_ignored() {
        let mut images = Assets::<Image>::default();
        let mut stream = LiveFrameStream::default();
        let bytes = jackdaw_pie_protocol::encode_frame(0, 0, 1, &[]);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&bytes).unwrap(),
        );
        assert!(stream.image.is_none());
        assert_eq!(stream.last_seq, 0);
    }

    #[test]
    fn clear_releases_the_asset() {
        let mut images = Assets::<Image>::default();
        let mut stream = LiveFrameStream::default();
        let b = frame_bytes(64, 64, 1, 1);
        apply_frame(
            &mut stream,
            &mut images,
            jackdaw_pie_protocol::decode_frame(&b).unwrap(),
        );
        let handle = stream.image.clone().unwrap();
        stream.clear(&mut images);
        assert!(stream.image.is_none());
        assert!(images.get(&handle).is_none());
        assert_eq!(stream.last_seq, 0);
    }
}
