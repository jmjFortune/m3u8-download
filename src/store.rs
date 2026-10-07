use crate::{
    config::DownloadSettings,
    model::{Task, now, safe_name, validate_url},
};
use rusqlite::{Connection, params};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

pub struct Store(Mutex<Connection>);
impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
        CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL, message TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL, attempt INTEGER NOT NULL DEFAULT 0, bytes INTEGER NOT NULL DEFAULT 0, output TEXT, headers TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
        CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);")?;
        // 下载进程随服务退出；仅恢复未结束的任务。
        db.execute("UPDATE tasks SET status='pending',message='Server restarted; task requeued' WHERE status IN ('resolving','downloading','verifying')", [])?;
        Ok(Self(Mutex::new(db)))
    }
    pub fn load_settings(&self) -> anyhow::Result<Option<DownloadSettings>> {
        use rusqlite::OptionalExtension;
        let value: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(Into::into)
    }
    pub fn save_settings(&self, settings: &DownloadSettings) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO settings(id,value) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }
    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
        let headers: String = r.get(10)?;
        Ok(Task {
            id: r.get(0)?,
            url: r.get(1)?,
            name: r.get(2)?,
            status: r.get(3)?,
            message: r.get(4)?,
            created: r.get(5)?,
            updated: r.get(6)?,
            attempt: r.get(7)?,
            bytes: r.get(8)?,
            output: r.get(9)?,
            headers: serde_json::from_str(&headers).unwrap_or_default(),
        })
    }
    pub fn list(&self) -> anyhow::Result<Vec<Task>> {
        let db = self.0.lock().unwrap();
        let mut q = db.prepare("SELECT * FROM tasks ORDER BY id DESC LIMIT 1000")?;
        Ok(q.query_map([], Self::row)?.collect::<Result<_, _>>()?)
    }
    pub fn get(&self, id: i64) -> anyhow::Result<Option<Task>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .0
            .lock()
            .unwrap()
            .query_row("SELECT * FROM tasks WHERE id=?", [id], Self::row)
            .optional()?)
    }
    pub fn add(
        &self,
        raw: &str,
        headers: &BTreeMap<String, String>,
    ) -> anyhow::Result<Option<i64>> {
        let u = validate_url(raw)?;
        let db = self.0.lock().unwrap();
        let mut query=db.prepare("SELECT status,output FROM tasks WHERE url=? AND status IN ('pending','resolving','downloading','verifying','ok','duplicate')")?;
        let rows = query.query_map([raw], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (status, output) = row?;
            if !matches!(status.as_str(), "ok" | "duplicate")
                || output.is_some_and(|p| Path::new(&p).is_file())
            {
                return Ok(None);
            }
        }
        drop(query);
        let name = safe_name(
            u.path_segments()
                .and_then(|mut s| s.next_back())
                .unwrap_or("video"),
        );
        db.execute("INSERT INTO tasks(url,name,status,message,created,updated,headers) VALUES(?,?,'pending','Queued',?,?,?)", params![raw,name,now(),now(),serde_json::to_string(headers)?])?;
        Ok(Some(db.last_insert_rowid()))
    }
    pub fn claim(&self) -> anyhow::Result<Option<Task>> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        use rusqlite::OptionalExtension;
        let t = tx
            .query_row(
                "SELECT * FROM tasks WHERE status='pending' ORDER BY id LIMIT 1",
                [],
                Self::row,
            )
            .optional()?;
        if let Some(ref t) = t {
            tx.execute(
                "UPDATE tasks SET status='resolving',message='Resolving the video page',updated=? WHERE id=?",
                params![now(), t.id],
            )?;
        }
        tx.commit()?;
        Ok(t)
    }
    pub fn update(
        &self,
        id: i64,
        status: &str,
        msg: &str,
        attempt: u16,
        bytes: u64,
        output: Option<&str>,
    ) -> anyhow::Result<()> {
        self.0.lock().unwrap().execute("UPDATE tasks SET status=?,message=?,updated=?,attempt=?,bytes=?,output=? WHERE id=? AND status!='cancelled'",params![status,msg,now(),attempt,bytes,output,id])?;
        Ok(())
    }
    pub fn cancel(&self, id: i64) -> anyhow::Result<bool> {
        Ok(self.0.lock().unwrap().execute("UPDATE tasks SET status='cancelled',message='Cancelled',updated=? WHERE id=? AND status IN ('pending','resolving','downloading','verifying')",params![now(),id])?>0)
    }
    pub fn retry(&self, id: i64) -> anyhow::Result<bool> {
        Ok(self.0.lock().unwrap().execute("UPDATE tasks SET status='pending',message='Requeued',updated=?,attempt=0,bytes=0,output=NULL WHERE id=? AND status IN ('fail','cancelled')",params![now(),id])?>0)
    }
    pub fn delete(&self, id: i64) -> anyhow::Result<bool> {
        use rusqlite::OptionalExtension;
        let db = self.0.lock().unwrap();
        let status: Option<String> = db
            .query_row("SELECT status FROM tasks WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(status) = status else {
            return Ok(false);
        };
        anyhow::ensure!(
            matches!(status.as_str(), "ok" | "duplicate" | "fail" | "cancelled"),
            "Cancel the task before deleting its record"
        );
        Ok(db.execute("DELETE FROM tasks WHERE id=?", [id])? > 0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_and_exact_url_dedup() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("db");
        let s = Store::open(&p).unwrap();
        let h = BTreeMap::new();
        let id = s.add("https://example.com/play?a=1", &h).unwrap().unwrap();
        assert!(s.add("https://example.com/play?a=1", &h).unwrap().is_none());
        assert!(s.add("https://example.com/play?a=2", &h).unwrap().is_some());
        s.claim().unwrap();
        drop(s);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get(id).unwrap().unwrap().status, "pending");
        s.cancel(id).unwrap();
        s.update(id, "ok", "late result", 1, 0, None).unwrap();
        assert_eq!(s.get(id).unwrap().unwrap().status, "cancelled");
        assert!(s.retry(id).unwrap());
    }
}
