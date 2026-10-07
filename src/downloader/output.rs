use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use time::{Date, Month, OffsetDateTime, UtcOffset};
use tokio_util::sync::CancellationToken;

fn day_name(now: OffsetDateTime) -> String {
    now.to_offset(UtcOffset::from_hms(8, 0, 0).unwrap())
        .date()
        .to_string()
}

fn is_day_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return false;
    }
    let year = name[..4].parse::<i32>().unwrap();
    let month = name[5..7].parse::<u8>().unwrap();
    let day = name[8..].parse::<u8>().unwrap();
    Month::try_from(month)
        .ok()
        .is_some_and(|month| Date::from_calendar_date(year, month, day).is_ok())
}

pub(crate) async fn publish(
    root: &Path,
    filename: &str,
    source: &Path,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    publish_at(root, filename, source, cancel, OffsetDateTime::now_utc()).await
}

async fn publish_at(
    root: &Path,
    filename: &str,
    source: &Path,
    cancel: &CancellationToken,
    now: OffsetDateTime,
) -> Result<PathBuf> {
    anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
    let directory = root.join(day_name(now));
    tokio::fs::create_dir_all(&directory)
        .await
        .context("Could not create today's download directory")?;
    let staged = tempfile::Builder::new()
        .prefix(".pagecatch-")
        .suffix(".part")
        .tempfile_in(&directory)
        .context("Today's download directory is not writable")?;
    // The staging file shares the destination volume; its guard also cleans up failures.
    tokio::fs::copy(source, staged.path())
        .await
        .context("Could not copy the verified video to its save directory")?;
    anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
    let destination = directory.join(filename);
    staged
        .persist(&destination)
        .context("Could not publish the verified video")?;
    Ok(destination)
}

pub(super) async fn video_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut folders = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = folders.pop() {
        let mut entries = tokio::fs::read_dir(&folder).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            let path = entry.path();
            if kind.is_file() && path.extension().is_some_and(|s| s == "mp4") {
                files.push(path);
            } else if folder == root
                && kind.is_dir()
                && entry.file_name().to_str().is_some_and(is_day_name)
            {
                folders.push(path);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Time;

    fn utc(year: i32, month: Month, day: u8, hour: u8, minute: u8, second: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::from_hms(hour, minute, second).unwrap(),
        )
    }

    #[test]
    fn folders_follow_beijing_midnight_month_year_and_leap_day() {
        for (instant, day) in [
            (utc(2026, Month::October, 6, 15, 59, 59), "2026-10-06"),
            (utc(2026, Month::October, 6, 16, 0, 0), "2026-10-07"),
            (utc(2026, Month::January, 31, 16, 0, 0), "2026-02-01"),
            (utc(2026, Month::December, 31, 16, 0, 0), "2027-01-01"),
            (utc(2028, Month::February, 28, 16, 0, 0), "2028-02-29"),
        ] {
            assert_eq!(day_name(instant), day);
        }
    }

    #[tokio::test]
    async fn creates_and_reuses_daily_directory_with_no_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("verified.mp4");
        tokio::fs::write(&source, b"verified media").await.unwrap();
        let root = dir.path().join("资料").join("Videos");
        let cancel = CancellationToken::new();
        let before = utc(2026, Month::October, 6, 15, 59, 59);
        for filename in ["1-first.mp4", "2-second.mp4"] {
            let file = publish_at(&root, filename, &source, &cancel, before)
                .await
                .unwrap();
            assert_eq!(file, root.join("2026-10-06").join(filename));
            assert_eq!(tokio::fs::read(file).await.unwrap(), b"verified media");
        }
        let after = utc(2026, Month::October, 6, 16, 0, 0);
        let file = publish_at(&root, "3-next-day.mp4", &source, &cancel, after)
            .await
            .unwrap();
        assert_eq!(file, root.join("2026-10-07/3-next-day.mp4"));
        assert_eq!(
            std::fs::read_dir(root.join("2026-10-06")).unwrap().count(),
            2
        );
        assert_eq!(
            std::fs::read_dir(root.join("2026-10-07")).unwrap().count(),
            1
        );
    }

    #[tokio::test]
    async fn cancellation_copy_and_publish_failures_leave_no_partial_video() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Videos");
        let source = dir.path().join("source.mp4");
        let now = utc(2026, Month::October, 6, 0, 0, 0);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            publish_at(&root, "video.mp4", &source, &cancel, now)
                .await
                .is_err()
        );
        assert!(!root.exists());
        let cancel = CancellationToken::new();
        assert!(
            publish_at(&root, "video.mp4", &source, &cancel, now)
                .await
                .is_err()
        );
        let day = root.join("2026-10-06");
        assert_eq!(std::fs::read_dir(&day).unwrap().count(), 0);
        tokio::fs::write(&source, b"verified media").await.unwrap();
        tokio::fs::create_dir(day.join("video.mp4")).await.unwrap();
        assert!(
            publish_at(&root, "video.mp4", &source, &cancel, now)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(day).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn dedup_candidates_include_legacy_and_previous_days_only() {
        let dir = tempfile::tempdir().unwrap();
        let expected = ["legacy.mp4", "2026-10-04/old.mp4", "2026-10-06/new.mp4"];
        for filename in expected.into_iter().chain([
            "2026-99-99/invalid.mp4",
            "other/ignored.mp4",
            "2026-10-06/nested/ignored.mp4",
            "2026-10-06/.partial.part",
        ]) {
            let file = dir.path().join(filename);
            tokio::fs::create_dir_all(file.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(file, b"fixture").await.unwrap();
        }
        let mut actual = video_files(dir.path()).await.unwrap();
        actual.sort();
        let mut expected = expected.map(|filename| dir.path().join(filename));
        expected.sort();
        assert_eq!(actual, expected);
    }
}
