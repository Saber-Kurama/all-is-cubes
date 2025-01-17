//! Data import and export between [`all_is_cubes`] types and other data formats.
//!
//! Currently supported formats:
//!
//! | Format              | Extension         | Import  | Export  | Caveats |
//! |---------------------|-------------------|:-------:|:-------:|---------|
//! | All is Cubes native | `.alliscubesjson` | **Yes** | **Yes** | Version compatibility not yet guaranteed. |
//! | MagicaVoxel `.vox`  | `.vox`            | **Yes** | **Yes** | Materials, scenes, and layers are ignored. |
//! | [glTF 2.0]          | `.gltf`           | —       | **Yes** | Textures are not yet implemented. Output is suitable for rendering but not necessarily editing due to combined meshes. |
//! | [STL]               | `.stl`            | —       | **Yes** | Meshes are not necessarily “manifold”/“watertight”. |
//!
//! [glTF 2.0]: https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html
//! [STL]: <https://en.wikipedia.org/wiki/STL_(file_format)>

// Basic lint settings, which should be identical across all all-is-cubes project crates.
// This list is sorted.
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::needless_update)]
#![allow(clippy::single_match)]
#![deny(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::cast_lossless)]
#![warn(clippy::doc_markdown)]
#![warn(clippy::exhaustive_enums)]
#![warn(clippy::exhaustive_structs)]
#![warn(clippy::modulo_arithmetic)]
#![warn(clippy::return_self_not_must_use)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::uninlined_format_args)]
#![warn(clippy::unnecessary_self_imports)]
#![warn(clippy::wrong_self_convention)]
#![warn(explicit_outlives_requirements)]
#![warn(missing_debug_implementations)]
#![warn(noop_method_call)]
#![warn(trivial_numeric_casts)]
#![warn(unused_extern_crates)]
#![warn(unused_lifetimes)]
// Lenience for tests.
#![cfg_attr(test,
    allow(clippy::float_cmp), // deterministic tests
    allow(clippy::redundant_clone), // prefer regularity over efficiency
)]
// #![warn(unused_crate_dependencies)]  // noisy for dev-dependencies; enable sometimes for review

// Crate-specific lint settings.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_core::future::BoxFuture;

use all_is_cubes::block::{self, BlockDef};
use all_is_cubes::space::Space;
use all_is_cubes::universe::{self, PartialUniverse, URef, Universe};
use all_is_cubes::util::YieldProgress;

pub mod file;
pub mod gltf;
mod mv;
use mv::load_dot_vox;
mod native;
mod stl;

#[cfg(test)]
mod tests;

/// Load a [`Universe`] described by the given file (of guessed format).
///
/// TODO: Make a from-bytes version of this.
pub async fn load_universe_from_file(
    progress: YieldProgress,
    file: Arc<dyn file::Fileish>,
) -> Result<Universe, ImportError> {
    // TODO: use extension, if any, for format detection
    let bytes = file.read().map_err(|error| ImportError {
        source_path: file.display_full_path(),
        detail: ImportErrorKind::Read { path: None, error },
    })?;

    let (mut universe, save_format) = if bytes.starts_with(b"{") {
        // Assume it's JSON. Furthermore, assume it's ours.
        (
            native::import_native_json(progress, &bytes, &*file)?,
            Some(ExportFormat::AicJson),
        )
    } else if bytes.starts_with(b"VOX ") {
        (
            load_dot_vox(progress, &bytes)
                .await
                .map_err(|error| ImportError {
                    source_path: file.display_full_path(),
                    detail: ImportErrorKind::Parse(Box::new(error)),
                })?,
            Some(ExportFormat::DotVox),
        )
    } else {
        return Err(ImportError {
            source_path: file.display_full_path(),
            detail: ImportErrorKind::UnknownFormat {},
        });
    };

    universe.whence = Arc::new(PortWhence { file, save_format });

    Ok(universe)
}

/// Export data specified by an [`ExportSet`] to a file on disk.
///
/// If the format requires multiple files, then they will be named with hyphenated suffixes
/// before the extension; i.e. "foo.gltf" becomes "foo-bar.gltf".
///
/// TODO: Generalize this or add a parallel function for non-filesystem destinations.
pub async fn export_to_path(
    progress: YieldProgress,
    format: ExportFormat,
    source: ExportSet,
    destination: PathBuf,
) -> Result<(), crate::ExportError> {
    match format {
        ExportFormat::AicJson => native::export_native_json(progress, source, destination).await,
        ExportFormat::DotVox => {
            // TODO: async file IO?
            mv::export_dot_vox(progress, source, fs::File::create(destination)?).await
        }
        ExportFormat::Gltf => gltf::export_gltf(progress, source, destination).await,
        ExportFormat::Stl => stl::export_stl(progress, source, destination).await,
    }
}

/// Selection of the data to be exported.
#[derive(Clone, Debug)]
pub struct ExportSet {
    /// `PartialUniverse` is defined in the `all_is_cubes` crate so that it can get access
    /// to the same serialization helpers as `Universe` and be guaranteed to serialize the
    /// exact same way.
    contents: PartialUniverse,
}

impl ExportSet {
    /// Construct an [`ExportSet`] specifying exporting all members of the universe
    /// (insofar as that is possible).
    ///
    /// Any members added between the call to this function and the export operation will
    /// not be included; removals may cause errors.
    pub fn all_of_universe(universe: &Universe) -> Self {
        Self {
            contents: PartialUniverse::all_of(universe),
        }
    }

    /// Construct an [`ExportSet`] specifying exporting only the given [`BlockDef`]s.
    pub fn from_block_defs(block_defs: Vec<URef<BlockDef>>) -> Self {
        Self {
            contents: PartialUniverse::from_set(block_defs),
        }
    }

    /// Construct an [`ExportSet`] specifying exporting only the given [`Space`]s.
    pub fn from_spaces(spaces: Vec<URef<Space>>) -> Self {
        Self {
            contents: PartialUniverse::from_set(spaces),
        }
    }

    /// Calculate the file path to use supposing that we want to export one member to one file
    /// (as opposed to all members into one file).
    ///
    /// This has a suffix added for uniqueness (after the name but preserving the existing
    /// extension), based on the item's [`URef::name()`], if the [`ExportSet`] contains more
    /// than one item. If it contains only one item, then `base_path` is returned unchanged.
    pub(crate) fn member_export_path(
        &self,
        base_path: &Path,
        member: &dyn universe::URefErased,
    ) -> PathBuf {
        let mut path: PathBuf = base_path.to_owned();
        if self.contents.count() > 1 {
            let mut new_file_name: OsString =
                base_path.file_stem().expect("file name missing").to_owned();
            new_file_name.push("-");
            match member.name() {
                // TODO: validate member name as filename fragment
                universe::Name::Specific(s) => new_file_name.push(&*s),
                universe::Name::Anonym(n) => new_file_name.push(n.to_string()),
                universe::Name::Pending => todo!(),
            };
            new_file_name.push(".");
            new_file_name.push(base_path.extension().expect("extension missing"));

            path.set_file_name(new_file_name);
        }
        path
    }
}

/// Implementation of [`WhenceUniverse`] used for this library's formats.
#[derive(Debug)]
struct PortWhence {
    file: Arc<dyn file::Fileish>,
    save_format: Option<ExportFormat>,
}

impl all_is_cubes::save::WhenceUniverse for PortWhence {
    fn document_name(&self) -> Option<String> {
        Some(self.file.document_name())
    }

    fn can_load(&self) -> bool {
        false
    }

    fn can_save(&self) -> bool {
        // TODO: implement this along with save()
        #[allow(unused)]
        let _ = self.save_format;
        false
    }

    fn load(
        &self,
        progress: YieldProgress,
    ) -> BoxFuture<'static, Result<Universe, Box<dyn std::error::Error + Send + Sync>>> {
        let file = self.file.clone();
        Box::pin(async move { Ok(load_universe_from_file(progress, file).await?) })
    }

    fn save(
        &self,
        universe: &Universe,
        progress: YieldProgress,
    ) -> BoxFuture<'static, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        // TODO: in order to implement this we need to be able to write to a `Fileish`
        // or have an accompanying destination
        let _ = (universe, progress, self.save_format);
        todo!();
    }
}

/// File formats that All is Cubes data can be exported to.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ExportFormat {
    /// Native format: JSON-encoded All is Cubes universe serialization.
    AicJson,

    /// [MagicaVoxel `.vox`][vox] file.
    ///
    /// TODO: document version details and export limitations
    ///
    /// [vox]: https://github.com/ephtracy/voxel-model/blob/master/MagicaVoxel-file-format-vox.txt
    DotVox,

    /// [glTF 2.0] format (`.gltf` JSON with auxiliary files).
    ///
    /// TODO: document capabilities
    ///
    /// TODO: document how auxiliary files are handled
    ///
    /// TODO: support `.glb` binary format.
    ///
    /// [glTF 2.0]: https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html
    Gltf,

    /// [STL] format.
    ///
    /// Supports exporting block and space shapes without color.
    ///
    /// [STL]: <https://en.wikipedia.org/wiki/STL_(file_format)>
    Stl,
}

impl ExportFormat {
    /// Whether exporting to this format is capable of including [`Space`] light data.
    pub fn includes_light(self) -> bool {
        match self {
            ExportFormat::AicJson => true,
            ExportFormat::DotVox => false,
            ExportFormat::Gltf => false, // TODO: implement light
            ExportFormat::Stl => false,
        }
    }
}

/// Fatal errors that may be encountered during an import operation.
///
/// TODO: Define non-fatal export flaws reporting, and link to it here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("failed to import '{source_path}'")]
pub struct ImportError {
    /// The path, as produced by [`file::Fileish::display_full_path()`] or similar,
    /// of the file being imported. Note that this is the originally specified path
    /// and may differ from the path of a file the error is about (specified separately),
    /// in case of multi-file data formats.
    pub source_path: String,

    #[source]
    detail: ImportErrorKind,
}

/// Specific reason why an import operation failed.
/// Always contained within an [`ImportError`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImportErrorKind {
    /// An IO error occurred while reading the data to import.
    #[non_exhaustive]
    #[error("failed to read data from {path:?}")] // TODO: Better formatting
    Read {
        /// The path, as produced by [`file::Fileish::display_full_path()`] or similar,
        /// of the file which could not be read, if it is not identical to the
        /// [`ImportError::source_path`].
        path: Option<String>,

        /// The IO error that occurred while reading.
        error: std::io::Error,
    },

    /// The data did not match the expected format, or was invalid as defined by that format.
    #[non_exhaustive]
    #[error("failed to parse the data")]
    Parse(
        /// Format-specific details of the parse error.
        #[source]
        Box<dyn std::error::Error + Send + Sync>,
    ),

    /// The data is not in a supported format.
    #[non_exhaustive]
    #[error("the data is not in a recognized format")]
    UnknownFormat {},
}

/// Fatal errors that may be encountered during an export operation.
///
/// TODO: Define non-fatal export flaws reporting, and link to it here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExportError {
    /// IO error while writing the data to a file or stream.
    ///
    /// TODO: also represent file path if available
    #[error("could not write export data")]
    Write(#[from] std::io::Error),

    /// `RefError` while reading the data to be exported.
    #[error("could not read universe to be exported")]
    Read(#[from] universe::RefError),

    /// `EvalBlockError` while exporting a block definition.
    #[error("could not evaluate block")]
    Eval {
        /// Name of the item being exported.
        name: universe::Name,

        /// Error that occurred.
        #[source]
        error: block::EvalBlockError,
    },

    /// The requested [`ExportSet`] contained data that cannot be represented in the
    /// requested [`ExportFormat`].
    #[error("could not convert data to requested format: {reason}")]
    NotRepresentable {
        /// Name of the item being exported.
        name: Option<universe::Name>,
        /// The reason why it cannot be represented.
        reason: String,
    },
}
