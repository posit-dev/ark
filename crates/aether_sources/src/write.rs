use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::directories::aether_cache_dir;

pub(crate) fn write_to_cache(
    files: &Vec<(String, String)>,
    destination: &Path,
) -> anyhow::Result<()> {
    let cache = aether_cache_dir()?;
    std::fs::create_dir_all(&cache)?;
    let temporary_destination = tempfile::tempdir_in(cache)?;

    // Write each read-only file into the temporary directory
    for (name, content) in files {
        let absolute = temporary_destination.path().join(name);
        std::fs::write(&absolute, content)?;
        std::fs::set_permissions(&absolute, Permissions::from_mode(0o444))?;
    }

    // Now rename `temporary_destination` to `destination` to atomically move it into place
    std::fs::create_dir_all(destination)?;

    match std::fs::rename(temporary_destination.path(), destination) {
        Ok(()) => {
            // Consume `TempDir` without deleting the underlying directory, since
            // we promoted it to `destination`
            let _ = temporary_destination.into_path();
            Ok(())
        },
        Err(err) => {
            if let Err(err) = std::fs::remove_dir(destination) {
                log::error!(
                    "Failed to remove {destination} after failed rename: {err:?}",
                    destination = destination.display()
                );
            }
            Err(err.into())
        },
    }
}
