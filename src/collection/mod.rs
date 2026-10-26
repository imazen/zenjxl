//! JPEG XL presentations and raw layers for [`zencodec::CodecSet`].
//!
//! Register [`crate::collection::JxlCollectionConfig`] with [`zencodec::CodecSet::with_collection`].
//! Opening fixes the composition view before any identities are returned.
//! Input must be resident and immutable for the opened operation; this adapter
//! does not silently download or stage a positioned source. Returned images own
//! their F32 samples and actual output color interpretation.
//!
//! Presentations render spot colors. Raw layers retain unrendered spot channels,
//! native blend/reference facts and signed placement; they are not flattened
//! previews. Both preserve stored orientation. Regions and child discovery are
//! refused. Raw layer selection replays dependencies from the start.
//!
//! The two native discovery allowances below are not a total memory budget.
//! The shared input-byte cap is checked against the resident slice before parsing;
//! it does not admit or allocate that caller-owned storage.
//! Shared decode resource requirements and policy flags that have not been
//! qualified are refused before native work. Inspect the retained
//! [`zencodec::collection::CollectionOpenError`] cause for the unhandled fields.
//! Native source details may contain identifying names and are **not** approved
//! for metadata passthrough or proof that opaque metadata was removed.

mod layers;
mod output;
mod presentations;

pub use output::{JxlRawLayer, JxlSourceFacts};

use alloc::{boxed::Box, sync::Arc};
use core::{error::Error, fmt};
use zencodec::{
    At, ImageFormat, ResourceLimits, StopToken,
    collection::{
        Cause, CollectionError, CollectionFactory, CollectionOpenContext, CollectionOpenError,
        Composition, ErasedCollection, SelectionError,
    },
    decode::DecodePolicy,
    enough::Stop,
    source::DynReadAt,
    storage::StorageBudget,
};

/// Native discovery configuration for checked image collections.
///
/// Each selected decode gets a fresh native decoder. The frame-header bound is
/// applied to each decoder, including discovery and replay; it is not a shared
/// charge across arbitrary repeated selections. Use the shared operation's
/// discovery allowance to bound caller-driven discovery attempts as well.
#[derive(Clone, Copy, Debug)]
pub struct JxlCollectionConfig {
    limits: presentations::Limits,
}

impl JxlCollectionConfig {
    /// Bound retained image identities and native frame-header work separately.
    ///
    /// Hidden dependency frames count toward `max_frame_headers` even when they
    /// do not become visible presentations. Zero refuses the first corresponding
    /// item/work; it never means unlimited. Neither bound admits native workspace
    /// or retained image allocations on behalf of an application memory budget.
    pub const fn new(max_images: usize, max_frame_headers: u64) -> Self {
        Self {
            limits: presentations::Limits {
                images: max_images,
                frame_headers: max_frame_headers,
            },
        }
    }
}

impl CollectionFactory for JxlCollectionConfig {
    fn formats(&self) -> &[ImageFormat] {
        &[ImageFormat::Jxl]
    }

    fn open_with<'a>(
        &self,
        source: &'a mut dyn DynReadAt,
        context: CollectionOpenContext,
    ) -> Result<ErasedCollection<'a>, Cause> {
        let CollectionOpenContext {
            format,
            view,
            limits,
            policy,
            stop,
            output_storage,
            ..
        } = context;
        require_configuration(format, limits, policy)?;
        stop.check()
            .map_err(|reason| At::wrap(zencodec::CodecError::new(None, reason.into())))?;
        let bytes = DynReadAt::as_slice(source)
            .ok_or_else(|| At::wrap(CollectionOpenError::ResidentInputRequired))?;
        if let Some(limits) = limits {
            // The immutable resident span is authoritative. A source length
            // hint cannot admit a larger payload or reject a smaller one.
            limits
                .check_input_size(u64::try_from(bytes.len())?)
                .map_err(At::wrap)?;
        }
        let control = Control::new(stop, output_storage);
        match view {
            Composition::Composited => Ok(ErasedCollection::new(
                presentations::JxlItems::open_controlled(bytes, view, self.limits, control)?,
            )),
            Composition::Uncomposited => Ok(ErasedCollection::new(layers::JxlLayers::open(
                bytes,
                self.limits,
                control,
            )?)),
            _ => Err(Box::new(SelectionError::UnsupportedComposition)),
        }
    }
}

#[track_caller]
fn require_configuration(
    format: ImageFormat,
    limits: Option<ResourceLimits>,
    policy: Option<DecodePolicy>,
) -> Result<(), Cause> {
    if format != ImageFormat::Jxl {
        return Err(At::wrap(CollectionOpenError::NotRegistered(format)).into());
    }
    if let Some(mut unsupported) = limits {
        // Explicitly encode-only; opening a collection performs no encode.
        unsupported.max_output_bytes = None;
        // Checked against the actual resident slice before native parsing.
        unsupported.max_input_bytes = None;
        if unsupported != ResourceLimits::none() {
            return Err(At::wrap(CollectionOpenError::UnsupportedLimits { unsupported }).into());
        }
    }
    if let Some(unsupported) = policy
        && unsupported != DecodePolicy::none()
    {
        return Err(At::wrap(CollectionOpenError::UnsupportedDecodePolicy { unsupported }).into());
    }
    Ok(())
}

type Failure = Cause;

#[track_caller]
fn located(error: Failure) -> At<CollectionError> {
    CollectionError::located(error)
}

// Constant diagnostics only. Native and caller errors retain their original
// concrete cause and existing At trace through the shared error envelope.
#[derive(Debug)]
struct NativeState(&'static str);
impl NativeState {
    #[track_caller]
    fn other(message: &'static str) -> At<Self> {
        At::wrap(Self(message))
    }
}
impl fmt::Display for NativeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for NativeState {}

#[derive(Clone)]
struct Control {
    stop: StopToken,
    output_storage: Option<StorageBudget>,
}
impl Control {
    fn new(stop: StopToken, output_storage: Option<StorageBudget>) -> Self {
        Self {
            stop,
            output_storage,
        }
    }
    fn check(&self) -> Result<(), Cause> {
        self.stop
            .check()
            .map_err(|reason| At::wrap(zencodec::CodecError::new(None, reason.into())).into())
    }
    fn token(&self) -> Arc<dyn Stop> {
        Arc::new(self.stop.clone())
    }
}
