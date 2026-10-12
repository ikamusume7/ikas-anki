// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `prefs21.db` 读写：与桌面版 Anki 共用同一份 profile / meta 配置。
//!
//! 对照 `qt/aqt/profiles.py`：
//!
//! - 文件是 SQLite，表结构 `profiles(name text primary key collate nocase, data blob not null)`；
//! - `_global` 行的 `data` 是 `ProfileManager.meta`；
//! - 每个 profile 行（名字就是 profile 目录名，如 `User 1`）的 `data` 是
//!   `ProfileManager.profile`；
//! - `data` 是 `pickle.dumps(obj, protocol=4)` 的结果。
//!
//! 为什么直接读写这个文件、而不是用 rslib 的集合配置（`getConfigJson`）：
//! 集合配置存在 `collection.anki2` 内部，与桌面版的 `prefs21.db` 是两份互不相干
//! 的数据。外壳与桌面版交替使用时，用户会发现设置「丢失」。这里选择共用同一份文件。
//!
//! 读写时必须**保持 pickle 原值**：`ProfileManager._pickle()` 会把 `QByteArray`
//! 转成 `bytes` 再序列化，`mainWindowGeom` 这类项因此在 pickle 里是 `bytes`。
//! 如果先把整个 dict 转成 JSON 再写回，这些项就会被破坏 —— 所以只在「返回给前端」
//! 这一步才做 JSON 转换。

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::Value as Json;
use serde_pickle::HashableValue;
use serde_pickle::Value;

/// 配置所在的存储：profile 自身，还是 `_global` 行的 meta。
#[derive(Clone, Copy)]
pub enum Store {
    Profile,
    Meta,
}

impl Store {
    /// 对应的 `profiles.name`（对照 `ProfileManager.save()`）。
    fn row_name(self, profile: &str) -> &str {
        match self {
            Store::Profile => profile,
            Store::Meta => "_global",
        }
    }
}

/// 一个 `prefs21.db` 的访问句柄。
pub struct Prefs21 {
    path: PathBuf,
    profile: String,
}

impl Prefs21 {
    /// 定位数据库。`profile_dir` 是 profile 目录（如 `<base>/User 1`）：
    /// 数据库位于其父目录（`<base>/prefs21.db`），profile 名即目录名。
    ///
    /// 对照 `ProfileManager(base)` + `load(name)`。路径无法解析时返回 `None`，
    /// 调用方应回退到本地 JSON 存储。
    pub fn locate(profile_dir: &Path) -> Option<Self> {
        let base = profile_dir.parent()?;
        let profile = profile_dir.file_name()?.to_str()?.to_owned();
        Some(Self {
            path: base.join("prefs21.db"),
            profile,
        })
    }

    /// 读取某个配置项。对照 `mediasrv.get_setting_json(pm.profile.get)`：
    /// 项不存在时返回 `None`（前端会看到 `null`）。
    pub fn get(&self, store: Store, key: &str) -> Result<Option<Json>, String> {
        let Some(blob) = self.read(store)? else {
            return Ok(None);
        };
        let map = decode_dict(&blob)?;
        let Some(value) = map.get(&HashableValue::String(key.to_owned())) else {
            return Ok(None);
        };
        Ok(Some(to_json(value)?))
    }

    /// 写入某个配置项。对照 `mediasrv.set_setting_json(pm.profile.__setitem__)`。
    pub fn set(&self, store: Store, key: &str, value: &Json) -> Result<(), String> {
        let mut map = match self.read(store)? {
            Some(blob) => decode_dict(&blob)?,
            None => BTreeMap::new(),
        };
        map.insert(HashableValue::String(key.to_owned()), from_json(value)?);

        let bytes = serde_pickle::value_to_vec(&Value::Dict(map), serde_pickle::SerOptions::new())
            .map_err(|error| format!("序列化 pickle 失败: {error}"))?;

        let connection = self.connect()?;
        let name = store.row_name(&self.profile);
        // 对照 `ProfileManager.save()`：先 update。行不存在时（例如集合目录不是桌面版
        // 建过的 profile）再插入 —— 只写入我们用到的键，不伪造 `profileConf` 的默认值，
        // 因为桌面版一律用 `profile.get(key, default)` 读取。
        let updated = connection
            .execute(
                "update profiles set data = ? where name = ? collate nocase",
                rusqlite::params![bytes, name],
            )
            .map_err(|error| format!("写入 profiles 失败: {error}"))?;
        if updated == 0 {
            connection
                .execute(
                    "insert into profiles(name, data) values (?, ?)",
                    rusqlite::params![name, bytes],
                )
                .map_err(|error| format!("插入 profiles 失败: {error}"))?;
        }
        Ok(())
    }

    fn read(&self, store: Store) -> Result<Option<Vec<u8>>, String> {
        // 不主动创建：文件不存在说明用户还没用过桌面版，交由调用方回退
        if !self.path.exists() {
            return Ok(None);
        }

        let connection = self.connect()?;
        let name = store.row_name(&self.profile);
        let mut statement = connection
            .prepare("select cast(data as blob) from profiles where name = ? collate nocase")
            .map_err(|error| format!("查询 profiles 失败: {error}"))?;
        statement
            .query_row(rusqlite::params![name], |row| row.get::<_, Vec<u8>>(0))
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(format!("读取 profiles 失败: {other}")),
            })
    }

    /// 打开数据库（**读写**）。
    ///
    /// 读路径也用读写方式打开，而不是 `SQLITE_OPEN_READ_ONLY`：上一次写入如果被
    /// 中途打断（进程被杀、断电），文件旁边会留下热日志，需要 SQLite 做恢复，
    /// 而只读连接做不到这一点 —— 会直接报
    /// `attempt to write a readonly database`（实测：`prepare` 就失败）。
    /// 结果是所有 profile / meta 读取一起挂掉，页面还会弹出错误框。
    /// Qt 那边就是普通 `sqlite3.connect()`（读写），这里保持一致。
    fn connect(&self) -> Result<Connection, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败: {error}"))?;
        }
        let connection = Connection::open(&self.path)
            .map_err(|error| format!("打开 {} 失败: {error}", self.path.display()))?;
        // 桌面版可能正持有该文件，短暂等待而不是立刻失败
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|error| format!("设置 busy_timeout 失败: {error}"))?;
        // 对照 `_loadMeta()`：必要时建表
        connection
            .execute(
                "create table if not exists profiles \
                 (name text primary key collate nocase, data blob not null)",
                [],
            )
            .map_err(|error| format!("建表失败: {error}"))?;
        Ok(connection)
    }
}

/// 解析 `profiles.data`。对照 `ProfileManager._unpickle()`。
fn decode_dict(blob: &[u8]) -> Result<BTreeMap<HashableValue, Value>, String> {
    let value = serde_pickle::value_from_slice(blob, serde_pickle::DeOptions::new())
        .map_err(|error| format!("解析 pickle 失败: {error}"))?;
    match value {
        Value::Dict(map) => Ok(map),
        other => Err(format!("profiles.data 不是 dict，而是 {other:?}")),
    }
}

/// pickle → JSON。对照 mediasrv 用 orjson 序列化取出的值，因此只支持 JSON 能表达的
/// 类型：遇到 `bytes` 等会明确报错，而不是静默丢数据（Qt 版在这种情况下同样会失败）。
fn to_json(value: &Value) -> Result<Json, String> {
    Ok(match value {
        Value::None => Json::Null,
        Value::Bool(flag) => Json::Bool(*flag),
        Value::I64(int) => Json::Number((*int).into()),
        // Python 的 int 可以超出 i64；超出时退化为字符串，避免精度丢失
        Value::Int(big) => match big.to_string().parse::<i64>() {
            Ok(int) => Json::Number(int.into()),
            Err(_) => Json::String(big.to_string()),
        },
        Value::F64(float) => serde_json::Number::from_f64(*float)
            .map(Json::Number)
            .ok_or_else(|| format!("无法用 JSON 表示该浮点数: {float}"))?,
        Value::String(text) => Json::String(text.clone()),
        Value::Bytes(_) => {
            return Err("该项是二进制值，JSON 无法表示（Qt 版同样会失败）".to_string())
        }
        Value::List(items) | Value::Tuple(items) => {
            Json::Array(items.iter().map(to_json).collect::<Result<Vec<_>, _>>()?)
        }
        // set / frozen set 的元素是「可哈希值」，转回普通值后再递归
        Value::Set(items) | Value::FrozenSet(items) => Json::Array(
            items
                .iter()
                .map(|item| to_json(&item.clone().into_value()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Dict(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let HashableValue::String(key) = key else {
                    return Err(format!("配置里含有非字符串键: {key:?}"));
                };
                out.insert(key.clone(), to_json(item)?);
            }
            Json::Object(out)
        }
    })
}

/// JSON → pickle。前端只会写 JSON 兼容的值，因此这个映射是完备的。
fn from_json(value: &Json) -> Result<Value, String> {
    Ok(match value {
        Json::Null => Value::None,
        Json::Bool(flag) => Value::Bool(*flag),
        Json::Number(number) => {
            if let Some(int) = number.as_i64() {
                Value::I64(int)
            } else if let Some(float) = number.as_f64() {
                Value::F64(float)
            } else {
                return Err(format!("无法表示的数值: {number}"));
            }
        }
        Json::String(text) => Value::String(text.clone()),
        Json::Array(items) => {
            Value::List(items.iter().map(from_json).collect::<Result<Vec<_>, _>>()?)
        }
        Json::Object(map) => {
            let mut out = BTreeMap::new();
            for (key, item) in map {
                out.insert(HashableValue::String(key.clone()), from_json(item)?);
            }
            Value::Dict(out)
        }
    })
}
