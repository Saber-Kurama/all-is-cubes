use std::path::PathBuf;
use std::{fs, io};

use all_is_cubes::universe::Universe;
use all_is_cubes::util::YieldProgress;

use crate::file::Fileish;
use crate::{ExportError, ExportSet, ImportError, ImportErrorKind};

#[cfg(test)]
mod tests;

pub(crate) fn import_native_json(
    progress: YieldProgress,
    bytes: &[u8],
    file: &dyn Fileish,
) -> Result<Universe, ImportError> {
    let reader = ReadProgressAdapter::new(progress, bytes);
    serde_json::from_reader(reader).map_err(|error| ImportError {
        source_path: file.display_full_path(),
        detail: if error.is_eof() || error.is_io() {
            ImportErrorKind::Read {
                path: None,
                error: io::Error::new(io::ErrorKind::Other, error),
            }
        } else {
            ImportErrorKind::Parse(Box::new(error))
        },
    })
}

pub(crate) async fn export_native_json(
    progress: YieldProgress,
    source: ExportSet,
    destination: PathBuf,
) -> Result<(), ExportError> {
    // TODO: Spin off a blocking thread to perform this export
    let ExportSet { contents } = source;
    serde_json::to_writer(
        io::BufWriter::new(fs::File::create(destination)?),
        &contents,
    )
    .map_err(|error| {
        // TODO: report non-IO errors distinctly
        ExportError::Write(io::Error::new(io::ErrorKind::Other, error))
    })?;
    progress.finish().await;
    Ok(())
}

struct ReadProgressAdapter<'a> {
    progress: YieldProgress,
    original_length: usize,
    last_report: usize,
    source: &'a [u8],
}

impl<'a> ReadProgressAdapter<'a> {
    pub fn new(progress: YieldProgress, source: &'a [u8]) -> Self {
        progress.progress_without_yield(0.0);
        Self {
            progress,
            original_length: source.len(),
            last_report: 0,
            source,
        }
    }

    fn report(&self) {
        self.progress
            .progress_without_yield(self.last_report as f32 / self.original_length as f32);
    }
}

impl io::Read for ReadProgressAdapter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let result = self.source.read(buf);

        let current_position = self.original_length - self.source.len();
        if (current_position - self.last_report) > 1024 * 1024 {
            self.last_report = current_position;
            self.report();
        }

        result
    }
}

impl Drop for ReadProgressAdapter<'_> {
    fn drop(&mut self) {
        self.report()
    }
}
