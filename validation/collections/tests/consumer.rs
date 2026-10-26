use std::{
    cell::Cell,
    convert::Infallible,
    error::Error,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use zencodec::{
    At, CodecSet, ImageFormat, ResourceLimits, StopToken, Unstoppable,
    collection::{
        CollectionOpenError, Composition, ImageRequest, OpenRequest, Rect, SelectionError,
    },
    decode::DecodePolicy,
    enough::{Stop, StopReason},
    image::{ImageOutput, Row},
    operation::{CollectionStage, Decision, PolicySet, Question},
    source::{DynReadAt, ReadAtShim, SliceReadAt},
};
use zenjxl::collection::{JxlCollectionConfig, JxlRawLayer, JxlSourceFacts};

fn fixture(name: &str) -> Vec<u8> {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    let root = ROOT.get_or_init(|| {
        codec_corpus::Corpus::new().unwrap().tar_url(
            "zencodec-jxl-8399f17c54c8138f65304bbc46efc80c269c7402",
            "https://codeload.github.com/imazen/zenjxl-decoder/tar.gz/8399f17c54c8138f65304bbc46efc80c269c7402",
        ).expect("pinned JXL corpus").join("zenjxl-decoder/resources/test")
    });
    std::fs::read(root.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn codecs() -> CodecSet {
    CodecSet::new().with_collection(JxlCollectionConfig::new(64, 64))
}
fn request(view: Composition) -> OpenRequest {
    OpenRequest::new(view, 512)
}
fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}
fn samples(image: &ImageOutput) -> Vec<Vec<u8>> {
    let mapped = image.map().unwrap();
    (0..mapped.plane_count())
        .map(|index| {
            let plane = mapped.plane(index).unwrap().unwrap();
            (0..plane.samples.dimensions().1)
                .flat_map(|y| {
                    let Row::Bytes(row) = plane.samples.row(y).unwrap() else {
                        panic!("expected native F32 bytes")
                    };
                    row.iter().copied()
                })
                .collect()
        })
        .collect()
}

#[test]
fn public_collection_retains_views_interpretation_and_owned_samples() {
    for (name, view, count) in [
        (
            "multiple_layers_noise_spline.jxl",
            Composition::Composited,
            1,
        ),
        (
            "multiple_layers_noise_spline.jxl",
            Composition::Uncomposited,
            5,
        ),
        (
            "conformance_test_images/cmyk_layers.jxl",
            Composition::Uncomposited,
            4,
        ),
    ] {
        let bytes = fixture(name);
        let mut source = ReadAtShim(SliceReadAt::new(&bytes));
        let registry = codecs();
        let address = Cell::new(0usize);
        let visits = Cell::new(0usize);
        let resolver = |question: Question<'_>| -> Result<Decision, At<Infallible>> {
            let Question::Image { image, facts, .. } = question else {
                panic!("wrong policy boundary")
            };
            assert_eq!(facts.placement, image.interpretation().placement);
            address.set(
                image
                    .with_pixels(|pixels| pixels.row(0).as_ptr() as usize)
                    .unwrap(),
            );
            visits.set(visits.get() + 1);
            Ok(Decision::UsePreset)
        };
        let mut images = registry
            .open_collection(
                ImageFormat::Jxl,
                &mut source,
                request(view),
                resolver,
                stop(),
            )
            .unwrap();
        drop(registry);
        let mut cursor = images.images().unwrap();
        let mut ids = Vec::new();
        while let Some(id) = images.next(&mut cursor).unwrap() {
            ids.push(id);
        }
        assert_eq!(ids.len(), count, "{name} {view:?}");
        assert_eq!(visits.get(), 0, "inspection must not publish pixels");
        let mut held = None;
        let mut expected = None;
        for index in (0..ids.len()).rev().chain([0]) {
            let description = images.describe(&ids[index]).unwrap();
            let image = images.decode(&ids[index], &ImageRequest::new()).unwrap();
            assert_eq!(
                address.get(),
                image
                    .with_pixels(|pixels| pixels.row(0).as_ptr() as usize)
                    .unwrap(),
                "policy must approve the returned allocation"
            );
            let interpretation = image.interpretation();
            assert_eq!(
                description.stored_size,
                (
                    interpretation.geometry.canvas_width,
                    interpretation.geometry.canvas_height
                )
            );
            assert_eq!(description.stored_orientation, interpretation.orientation);
            assert_eq!(description.placement, interpretation.placement);
            let native = interpretation
                .source_details
                .get::<JxlSourceFacts>()
                .unwrap();
            assert_eq!(native.frame_index, index);
            assert_eq!(
                native.extra_details.len(),
                native.header.extra_channels.len()
            );
            if view == Composition::Uncomposited {
                let layer = interpretation.source_details.get::<JxlRawLayer>().unwrap();
                assert_eq!(layer.image, ids[index]);
                assert_eq!(
                    description.placement.unwrap().origin,
                    (
                        i64::from(layer.native.origin.0),
                        i64::from(layer.native.origin.1)
                    )
                );
                if name.contains("cmyk") {
                    image
                        .with_pixels(|p| {
                            assert_eq!(
                                p.descriptor().format,
                                zenpixels::PixelFormat::CmykF32Inverted
                            )
                        })
                        .unwrap();
                }
            } else {
                assert!(!interpretation.source_details.contains::<JxlRawLayer>());
            }
            if index == 0 {
                let actual = samples(&image);
                if let Some(expected) = &expected {
                    assert_eq!(
                        &actual, expected,
                        "reverse selection/replay changed sample bits"
                    );
                }
                expected = Some(actual);
            }
            held = Some(image);
        }
        assert_eq!(visits.get(), count + 1);
        drop(images);
        drop(bytes);
        assert_eq!(samples(&held.unwrap()), expected.unwrap());
    }
}

#[test]
fn all_stored_orientations_remain_explicit_in_both_views() {
    for (index, name) in [
        "orientation1_identity.jxl",
        "orientation2_flip_horizontal.jxl",
        "orientation3_rotate_180.jxl",
        "orientation4_flip_vertical.jxl",
        "orientation5_transpose.jxl",
        "orientation6_rotate_90_cw.jxl",
        "orientation7_anti_transpose.jxl",
        "orientation8_rotate_90_ccw.jxl",
    ]
    .into_iter()
    .enumerate()
    {
        let bytes = fixture(name);
        for view in [Composition::Composited, Composition::Uncomposited] {
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            let mut images = codecs()
                .open_collection(
                    ImageFormat::Jxl,
                    &mut source,
                    request(view),
                    PolicySet::new(),
                    stop(),
                )
                .unwrap();
            let mut cursor = images.images().unwrap();
            let id = images.next(&mut cursor).unwrap().unwrap();
            let description = images.describe(&id).unwrap();
            assert_eq!(
                description.stored_orientation,
                zenpixels::Orientation::from_exif(index as u8 + 1)
            );
            let output = images.decode(&id, &ImageRequest::new()).unwrap();
            assert_eq!(
                description.stored_orientation,
                output.interpretation().orientation
            );
            assert_eq!(
                description.stored_size,
                (
                    output.interpretation().geometry.canvas_width,
                    output.interpretation().geometry.canvas_height
                )
            );
            assert!(images.next(&mut cursor).unwrap().is_none());
        }
    }
}

// Every call is observable. A refusal must not inspect the source as a fallback.
struct NoRead;
impl DynReadAt for NoRead {
    fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), zencodec::collection::Cause> {
        panic!("unexpected exact source read")
    }
    fn len(&mut self) -> Result<u64, zencodec::collection::Cause> {
        panic!("unexpected length query")
    }
    fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize, zencodec::collection::Cause> {
        panic!("unexpected source read")
    }
}

#[test]
fn unhandled_requirements_refuse_before_io_with_typed_located_causes() {
    let limits = ResourceLimits::none().with_max_width(1).with_max_memory(1);
    let error = codecs()
        .with_limits(limits)
        .open_collection(
            ImageFormat::Jxl,
            &mut NoRead,
            request(Composition::Composited),
            PolicySet::new(),
            stop(),
        )
        .err()
        .unwrap();
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::UnsupportedLimits {
            unsupported: limits
        })
    );
    assert!(zencodec::find_cause::<At<CollectionOpenError>>(&error).is_some());
    let policy = DecodePolicy::none().with_allow_animation(false);
    let error = codecs()
        .with_decode_policy(policy)
        .open_collection(
            ImageFormat::Jxl,
            &mut NoRead,
            request(Composition::Composited),
            PolicySet::new(),
            stop(),
        )
        .err()
        .unwrap();
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::UnsupportedDecodePolicy {
            unsupported: policy
        })
    );
    let error = codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut NoRead,
            request(Composition::Composited),
            PolicySet::new(),
            stop(),
        )
        .err()
        .unwrap();
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::ResidentInputRequired)
    );
}

struct Resident<'a> {
    bytes: &'a [u8],
    hint: Option<u64>,
}
impl DynReadAt for Resident<'_> {
    fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), zencodec::collection::Cause> {
        panic!("resident input must not be read again")
    }
    fn len(&mut self) -> Result<u64, zencodec::collection::Cause> {
        panic!("resident admission must not query another length")
    }
    fn len_hint(&self) -> Option<u64> {
        self.hint
    }
    fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize, zencodec::collection::Cause> {
        panic!("resident input must not be read again")
    }
    fn as_slice(&self) -> Option<&[u8]> {
        Some(self.bytes)
    }
}

#[test]
fn resident_input_limit_uses_actual_bytes_before_native_parsing() {
    use zencodec::LimitExceeded;
    for view in [Composition::Composited, Composition::Uncomposited] {
        for hint in [None, Some(0), Some(u64::MAX)] {
            // A native parse would fail for a different reason. The input-size
            // refusal must win before the decoder inspects these bytes.
            for max in [0, 1, 2] {
                let mut source = Resident {
                    bytes: b"bad",
                    hint,
                };
                let error = codecs()
                    .with_limits(ResourceLimits::none().with_max_input_bytes(max))
                    .open_collection(
                        ImageFormat::Jxl,
                        &mut source,
                        request(view),
                        PolicySet::new(),
                        stop(),
                    )
                    .err()
                    .expect("oversize resident input was admitted");
                assert_eq!(
                    zencodec::find_cause::<LimitExceeded>(&error),
                    Some(&LimitExceeded::InputSize { actual: 3, max })
                );
                assert!(zencodec::find_cause::<At<LimitExceeded>>(&error).is_some());
                assert_eq!(error.error().stage, CollectionStage::Open);
            }
        }
    }
    // Known fields are excluded from an unhandled-configuration report; mixing
    // in an unqualified requirement still refuses without touching the source.
    let unsupported = ResourceLimits::none().with_max_memory(1);
    let error = codecs()
        .with_limits(unsupported.with_max_input_bytes(0))
        .open_collection(
            ImageFormat::Jxl,
            &mut NoRead,
            request(Composition::Composited),
            PolicySet::new(),
            stop(),
        )
        .err()
        .unwrap();
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::UnsupportedLimits { unsupported })
    );
    let error = codecs()
        .with_limits(ResourceLimits::none().with_max_input_bytes(1))
        .open_collection(
            ImageFormat::Jxl,
            &mut NoRead,
            request(Composition::Composited),
            PolicySet::new(),
            stop(),
        )
        .err()
        .unwrap();
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::ResidentInputRequired)
    );
}

#[test]
fn resident_input_limit_boundary_preserves_both_views_and_native_samples() {
    let bytes = fixture("named_frame_test.jxl");
    for view in [Composition::Composited, Composition::Uncomposited] {
        let mut source = ReadAtShim(SliceReadAt::new(&bytes));
        let mut original = codecs()
            .open_collection(
                ImageFormat::Jxl,
                &mut source,
                request(view),
                PolicySet::new(),
                stop(),
            )
            .unwrap();
        let expected = original.first_image(&ImageRequest::new()).unwrap().unwrap();
        for hint in [Some(0), Some(u64::MAX)] {
            for allowance in [bytes.len() as u64, bytes.len() as u64 + 1] {
                let mut source = Resident {
                    bytes: &bytes,
                    hint,
                };
                let mut images = codecs()
                    .with_limits(ResourceLimits::none().with_max_input_bytes(allowance))
                    .open_collection(
                        ImageFormat::Jxl,
                        &mut source,
                        request(view),
                        PolicySet::new(),
                        stop(),
                    )
                    .unwrap();
                let output = images.first_image(&ImageRequest::new()).unwrap().unwrap();
                assert_eq!(samples(&output), samples(&expected));
                assert_eq!(
                    output.interpretation().geometry,
                    expected.interpretation().geometry
                );
                assert_eq!(
                    output.interpretation().placement,
                    expected.interpretation().placement
                );
            }
        }
    }
}

#[test]
fn foreign_id_child_and_region_refusals_leave_valid_selection_available() {
    let bytes = fixture("conformance_test_images/cmyk_layers.jxl");
    let mut first_source = ReadAtShim(SliceReadAt::new(&bytes));
    let mut second_source = ReadAtShim(SliceReadAt::new(&bytes));
    let mut first = codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut first_source,
            request(Composition::Uncomposited),
            PolicySet::new(),
            stop(),
        )
        .unwrap();
    let mut second = codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut second_source,
            request(Composition::Uncomposited),
            PolicySet::new(),
            stop(),
        )
        .unwrap();
    let mut cursor = first.images().unwrap();
    let id = first.next(&mut cursor).unwrap().unwrap();
    for error in [
        second.describe(&id).unwrap_err(),
        second.decode(&id, &ImageRequest::new()).unwrap_err(),
    ] {
        assert_eq!(
            zencodec::find_cause::<SelectionError>(&error),
            Some(&SelectionError::ForeignImage)
        );
    }
    let error = second.next(&mut cursor).unwrap_err();
    assert_eq!(
        zencodec::find_cause::<SelectionError>(&error),
        Some(&SelectionError::ForeignCursor)
    );
    let error = first.children(&id).err().unwrap();
    assert_eq!(
        zencodec::find_cause::<SelectionError>(&error),
        Some(&SelectionError::UnsupportedChildren)
    );
    let region = ImageRequest::new().with_region(Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    });
    let error = first.decode(&id, &region).unwrap_err();
    assert_eq!(
        zencodec::find_cause::<SelectionError>(&error),
        Some(&SelectionError::UnsupportedRegion)
    );
    assert!(first.decode(&id, &ImageRequest::new()).is_ok());
}

#[derive(Clone)]
struct CheckStop {
    count: Arc<AtomicUsize>,
    at: usize,
    reason: StopReason,
}
impl Stop for CheckStop {
    fn check(&self) -> Result<(), StopReason> {
        if self.count.fetch_add(1, Ordering::SeqCst) >= self.at {
            Err(self.reason)
        } else {
            Ok(())
        }
    }
}

#[test]
fn cancellation_and_timeout_cover_all_observed_publication_seams() {
    let bytes = fixture("orientation6_rotate_90_cw.jxl");
    for view in [Composition::Composited, Composition::Uncomposited] {
        let run = |token: CheckStop| {
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            let mut images = codecs().open_collection(
                ImageFormat::Jxl,
                &mut source,
                request(view),
                PolicySet::new(),
                StopToken::new(token),
            )?;
            let result = images.first_image(&ImageRequest::new());
            if let Err(error) = &result {
                let reason = error
                    .error()
                    .stop_reason()
                    .expect("injected stop is retained");
                let next = images.images().err().expect("stopped operation resumed");
                assert_eq!(next.error().stage, CollectionStage::Finished);
                assert_eq!(next.error().stop_reason(), Some(reason));
            }
            result
        };
        let count = Arc::new(AtomicUsize::new(0));
        let result = run(CheckStop {
            count: count.clone(),
            at: usize::MAX,
            reason: StopReason::Cancelled,
        })
        .unwrap()
        .unwrap();
        assert!(!samples(&result).is_empty());
        let checkpoints = count.load(Ordering::SeqCst);
        assert!(
            checkpoints > 5 && checkpoints < 4096,
            "unexpected checkpoints {checkpoints}"
        );
        for reason in [StopReason::Cancelled, StopReason::TimedOut] {
            for at in 0..checkpoints {
                let error = run(CheckStop {
                    count: Arc::new(AtomicUsize::new(0)),
                    at,
                    reason,
                })
                .unwrap_err();
                assert_eq!(
                    error.error().stop_reason(),
                    Some(reason),
                    "{view:?} seam {at}"
                );
                // A stop observed inside publication retains the Policy stage;
                // stop_reason is the independent, reliable cancellation fact.
            }
        }
    }
}

#[derive(Debug)]
struct PrivatePolicyError;
impl std::fmt::Display for PrivatePolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("private policy payload")
    }
}
impl Error for PrivatePolicyError {}

#[test]
fn callback_errors_keep_location_and_do_not_leak_payloads_or_bypass_policy() {
    let bytes = fixture("conformance_test_images/cmyk_layers.jxl");
    let mut source = ReadAtShim(SliceReadAt::new(&bytes));
    let resolver = |_: Question<'_>| -> Result<Decision, At<PrivatePolicyError>> {
        Err(At::wrap(PrivatePolicyError))
    };
    let mut images = codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut source,
            request(Composition::Uncomposited),
            resolver,
            stop(),
        )
        .unwrap();
    let error = images.first_image(&ImageRequest::new()).unwrap_err();
    assert_eq!(error.error().stage, CollectionStage::Policy);
    assert!(zencodec::find_cause::<At<PrivatePolicyError>>(&error).is_some());
    assert!(!format!("{error} {error:?}").contains("private policy payload"));
    let mut source = ReadAtShim(SliceReadAt::new(&bytes));
    let mut images = codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut source,
            request(Composition::Uncomposited),
            PolicySet::new().allow_opaque_profiles(false),
            stop(),
        )
        .unwrap();
    assert_eq!(
        images
            .first_image(&ImageRequest::new())
            .unwrap_err()
            .error()
            .stage,
        CollectionStage::Policy
    );
}

#[test]
fn native_discovery_allowances_latch_failure_instead_of_reporting_exhaustion() {
    let bytes = fixture("multiple_layers_noise_spline.jxl");
    for (view, images, headers) in [
        (Composition::Composited, 64, 1),
        (Composition::Uncomposited, 1, 64),
    ] {
        let mut source = ReadAtShim(SliceReadAt::new(&bytes));
        let mut collection = CodecSet::new()
            .with_collection(JxlCollectionConfig::new(images, headers))
            .open_collection(
                ImageFormat::Jxl,
                &mut source,
                request(view),
                PolicySet::new(),
                stop(),
            )
            .unwrap();
        let mut cursor = collection.images().unwrap();
        let mut failure = None;
        for _ in 0..8 {
            match collection.next(&mut cursor) {
                Ok(Some(_)) => (),
                Ok(None) => panic!("allowance became successful exhaustion"),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let error = failure.expect("native work was not bounded");
        assert_eq!(error.error().stage, CollectionStage::Discovery);
        assert!(collection.next(&mut cursor).is_err(), "failed scan resumed");
    }
}

fn output_storage_size(image: &ImageOutput) -> (usize, usize) {
    let mapped = image.map().unwrap();
    let bytes = (0..mapped.plane_count())
        .map(|index| {
            let plane = mapped.plane(index).unwrap().unwrap();
            plane.samples.stride_bytes() * plane.samples.dimensions().1
        })
        .sum();
    (bytes, mapped.plane_count())
}
fn ordinary_first(bytes: &[u8], view: Composition) -> ImageOutput {
    let mut source = ReadAtShim(SliceReadAt::new(bytes));
    codecs()
        .open_collection(
            ImageFormat::Jxl,
            &mut source,
            request(view),
            PolicySet::new(),
            stop(),
        )
        .unwrap()
        .first_image(&ImageRequest::new())
        .unwrap()
        .expect("fixture has a first image")
        .into_output()
}

#[test]
fn output_storage_follows_last_clone_across_registry_drop_and_new_operations() {
    use zencodec::storage::{StorageBudget, StorageError};
    for name in [
        "named_frame_test.jxl",
        "conformance_test_images/cmyk_layers.jxl",
    ] {
        for view in [Composition::Composited, Composition::Uncomposited] {
            let bytes = fixture(name);
            let expected = ordinary_first(&bytes, view);
            let (size, objects) = output_storage_size(&expected);
            let storage = StorageBudget::new(size, objects);
            let config = JxlCollectionConfig::new(64, 64);
            let registry = CodecSet::new().with_collection(config);
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            let address = Cell::new(0usize);
            let resolver = |question: Question<'_>| -> Result<Decision, At<Infallible>> {
                let Question::Image { image, .. } = question else {
                    panic!("wrong policy boundary")
                };
                assert_eq!(
                    (storage.reserved_bytes(), storage.reserved_objects()),
                    (size, objects)
                );
                address.set(
                    image
                        .with_pixels(|p| p.as_strided_bytes().as_ptr() as usize)
                        .unwrap(),
                );
                assert_eq!(address.get() % core::mem::align_of::<f32>(), 0);
                Ok(Decision::UsePreset)
            };
            let mut opened = registry
                .open_collection(
                    ImageFormat::Jxl,
                    &mut source,
                    request(view).with_output_storage(storage.clone()),
                    resolver,
                    stop(),
                )
                .unwrap();
            drop(registry);
            let image = opened
                .first_image(&ImageRequest::new())
                .unwrap()
                .expect("fixture has a first image")
                .into_output();
            assert_eq!(samples(&image), samples(&expected));
            assert_eq!(
                image
                    .with_pixels(|p| p.as_strided_bytes().as_ptr() as usize)
                    .unwrap(),
                address.get()
            );
            let held = image.clone();
            drop(image);
            drop(opened);
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (size, objects)
            );

            let mut other_source = ReadAtShim(SliceReadAt::new(&bytes));
            let mut other = CodecSet::new()
                .with_collection(config)
                .open_collection(
                    ImageFormat::Jxl,
                    &mut other_source,
                    request(view).with_output_storage(storage.clone()),
                    PolicySet::new(),
                    stop(),
                )
                .unwrap();
            let mut cursor = other.images().unwrap();
            let id = other.next(&mut cursor).unwrap().unwrap();
            let error = other.decode(&id, &ImageRequest::new()).unwrap_err();
            assert!(zencodec::find_cause::<StorageError>(&error).is_some());
            assert!(!other.is_finished(), "allocation refusal is request-local");
            assert_eq!(samples(&held), samples(&expected));
            drop(held);
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (0, 0)
            );
            let next = other.decode(&id, &ImageRequest::new()).unwrap();
            assert_eq!(samples(&next), samples(&expected));
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (size, objects)
            );
            drop(next);
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (0, 0)
            );
        }
    }
}

#[test]
fn output_storage_partial_admission_refuses_without_unaccounted_fallback() {
    use zencodec::storage::{StorageBudget, StorageError};
    let bytes = fixture("conformance_test_images/cmyk_layers.jxl");
    for view in [Composition::Composited, Composition::Uncomposited] {
        let expected = ordinary_first(&bytes, view);
        let (size, objects) = output_storage_size(&expected);
        assert!(
            objects > 1,
            "fixture must exercise partial auxiliary allocation"
        );
        for (byte_limit, object_limit) in [
            (0, objects),
            (size - 1, objects),
            (size, 0),
            (size, objects - 1),
        ] {
            let storage = StorageBudget::new(byte_limit, object_limit);
            let registry = CodecSet::new().with_collection(JxlCollectionConfig::new(64, 64));
            let calls = Cell::new(0usize);
            let resolver = |_: Question<'_>| -> Result<Decision, At<Infallible>> {
                calls.set(calls.get() + 1);
                Ok(Decision::UsePreset)
            };
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            let mut opened = registry
                .open_collection(
                    ImageFormat::Jxl,
                    &mut source,
                    request(view).with_output_storage(storage.clone()),
                    resolver,
                    stop(),
                )
                .unwrap();
            let error = opened.first_image(&ImageRequest::new()).unwrap_err();
            let cause = zencodec::find_cause::<StorageError>(&error).unwrap();
            if byte_limit < size {
                assert!(matches!(cause, StorageError::Bytes { .. }));
            } else {
                assert!(matches!(cause, StorageError::Objects { .. }));
            }
            assert_eq!(calls.get(), 0, "refused allocation reached publication");
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (0, 0)
            );
        }
    }
}

#[derive(Clone)]
struct StopAfterOutput {
    storage: zencodec::storage::StorageBudget,
    observed: Arc<AtomicUsize>,
}
impl Stop for StopAfterOutput {
    fn check(&self) -> Result<(), StopReason> {
        if self.storage.reserved_bytes() > 0 {
            self.observed.fetch_add(1, Ordering::SeqCst);
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[test]
fn output_storage_releases_unpublished_samples_on_stop_decline_error_and_unwind() {
    use zencodec::{operation::Decline, storage::StorageBudget};
    let bytes = fixture("conformance_test_images/cmyk_layers.jxl");
    for view in [Composition::Composited, Composition::Uncomposited] {
        let expected = ordinary_first(&bytes, view);
        let (size, objects) = output_storage_size(&expected);
        let storage = StorageBudget::new(size, objects);
        let registry = CodecSet::new().with_collection(JxlCollectionConfig::new(64, 64));
        let observed = Arc::new(AtomicUsize::new(0));
        let token = StopToken::new(StopAfterOutput {
            storage: storage.clone(),
            observed: observed.clone(),
        });
        let mut source = ReadAtShim(SliceReadAt::new(&bytes));
        let mut opened = registry
            .open_collection(
                ImageFormat::Jxl,
                &mut source,
                request(view).with_output_storage(storage.clone()),
                PolicySet::new(),
                token,
            )
            .unwrap();
        let error = opened.first_image(&ImageRequest::new()).unwrap_err();
        assert_eq!(error.error().stop_reason(), Some(StopReason::Cancelled));
        assert!(opened.is_finished());
        assert!(observed.load(Ordering::SeqCst) > 0);
        assert_eq!(
            (storage.reserved_bytes(), storage.reserved_objects()),
            (0, 0)
        );

        for mode in 0..3 {
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            let resolver = |_: Question<'_>| -> Result<Decision, At<PrivatePolicyError>> {
                assert_eq!(
                    (storage.reserved_bytes(), storage.reserved_objects()),
                    (size, objects)
                );
                match mode {
                    0 => Ok(Decision::decline(Decline::ApplicationRule)),
                    1 => Err(At::wrap(PrivatePolicyError)),
                    _ => panic!("controlled policy unwind with admitted native output"),
                }
            };
            let mut opened = registry
                .open_collection(
                    ImageFormat::Jxl,
                    &mut source,
                    request(view).with_output_storage(storage.clone()),
                    resolver,
                    stop(),
                )
                .unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                opened.first_image(&ImageRequest::new())
            }));
            if mode == 2 {
                assert!(result.is_err());
                assert!(opened.is_finished());
            } else {
                let error = result.unwrap().unwrap_err();
                assert_eq!(error.error().stage, CollectionStage::Policy);
                if mode == 0 {
                    assert!(zencodec::find_cause::<Decline>(&error).is_some());
                } else {
                    assert!(zencodec::find_cause::<At<PrivatePolicyError>>(&error).is_some());
                }
            }
            assert_eq!(
                (storage.reserved_bytes(), storage.reserved_objects()),
                (0, 0)
            );
        }
    }
}

#[test]
fn operation_output_pools_are_independent_through_one_registry() {
    use zencodec::storage::{StorageBudget, StorageError};
    let bytes = fixture("conformance_test_images/cmyk_layers.jxl");
    for view in [Composition::Composited, Composition::Uncomposited] {
        let registry = codecs();
        let expected = ordinary_first(&bytes, view);
        let (size, objects) = output_storage_size(&expected);
        let first_pool = StorageBudget::new(size, objects);
        let second_pool = StorageBudget::new(size, objects);
        let required = request(view).with_output_storage(first_pool.clone());
        let decode = |request| {
            let mut source = ReadAtShim(SliceReadAt::new(&bytes));
            registry
                .open_collection(
                    ImageFormat::Jxl,
                    &mut source,
                    request,
                    PolicySet::new(),
                    stop(),
                )
                .unwrap()
                .first_image(&ImageRequest::new())
        };
        let first = decode(required.clone()).unwrap().unwrap();
        let full = decode(required).unwrap_err();
        assert!(zencodec::find_cause::<StorageError>(&full).is_some());
        let second = decode(request(view).with_output_storage(second_pool.clone()))
            .unwrap()
            .unwrap();
        // A request without a pool keeps ordinary allocation. Opening does not
        // mutate the shared registry or transfer another request's reservation.
        let ordinary = decode(request(view)).unwrap().unwrap();
        for image in [&first, &second, &ordinary] {
            assert_eq!(samples(image), samples(&expected));
        }
        assert_eq!(first_pool.reserved_bytes(), size);
        assert_eq!(second_pool.reserved_bytes(), size);
        drop(first);
        assert_eq!(
            (first_pool.reserved_bytes(), first_pool.reserved_objects()),
            (0, 0)
        );
        assert_eq!(second_pool.reserved_bytes(), size);
        drop(second);
        assert_eq!(
            (second_pool.reserved_bytes(), second_pool.reserved_objects()),
            (0, 0)
        );
        assert_eq!(samples(&ordinary), samples(&expected));
    }
}

#[test]
fn output_pool_does_not_satisfy_native_workspace_requirement() {
    use zencodec::storage::StorageBudget;
    let storage = StorageBudget::new(1024, 4);
    let limits = ResourceLimits::none().with_max_memory(1024);
    let mut source = Resident {
        bytes: b"not a JPEG XL stream",
        hint: None,
    };
    let error = codecs()
        .with_limits(limits)
        .open_collection(
            ImageFormat::Jxl,
            &mut source,
            request(Composition::Composited).with_output_storage(storage.clone()),
            PolicySet::new(),
            stop(),
        )
        .err()
        .expect("payload pool was mistaken for workspace admission");
    assert_eq!(
        zencodec::find_cause::<CollectionOpenError>(&error),
        Some(&CollectionOpenError::UnsupportedLimits {
            unsupported: limits
        })
    );
    assert_eq!(
        (storage.reserved_bytes(), storage.reserved_objects()),
        (0, 0)
    );
}
