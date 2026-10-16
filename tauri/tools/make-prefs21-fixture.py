# 造一份「桌面版写入的」prefs21.db，用来验证外壳的配置读写确实与桌面版共用同一份数据。
#
# 为什么需要这个小工具：`check-prefs21.mjs` 只能证明外壳的接口自洽，无法证明它读的
# 是桌面版的数据格式。这里用 Python 按 `qt/aqt/profiles.py` 的方式（SQLite +
# `pickle.dumps(protocol=4)`）写一份，再让外壳去读。
#
# 用法（在仓库根目录执行）：
#   py tauri/tools/make-prefs21-fixture.py init out/prefs21.db tauri-test
#   py tauri/tools/make-prefs21-fixture.py init out/prefs21.db "User 1" en_US   # 可选第 4 参：界面语言
#   py tauri/tools/make-prefs21-fixture.py show out/prefs21.db tauri-test
#   py tauri/tools/make-prefs21-fixture.py show out/prefs21.db _global
#
# 注意 base 目录就是 prefs21.db 所在目录：集合是 out/tauri-test/collection.anki2 时，
# profile 目录为 out/tauri-test、base 为 out，所以 profile 名取 tauri-test。

import json
import pickle
import sqlite3
import sys

CREATE = (
    "create table if not exists profiles "
    "(name text primary key collate nocase, data blob not null)"
)


def init(path: str, profile: str, lang: str = "zh_CN") -> None:
    conn = sqlite3.connect(path)
    conn.execute(CREATE)

    # 模拟桌面版的 profile：`mainWindowGeom` 是 QByteArray，被 _pickle() 转成 bytes。
    # 外壳重写整个 dict 时必须原样保留它，否则会破坏桌面版的窗口状态。
    profile_data = dict(
        numBackups=50,
        searchHistory=["deck:current", "tag:marked"],
        syncMedia=True,
        autoSync=True,
        allowHTML=False,
        importMode=1,
        mainWindowGeom=b"\x01\x02\x03\x04\xff",
        lastColour="#00f",
    )
    conn.execute(
        "insert or replace into profiles values (?, ?)",
        (profile, pickle.dumps(profile_data, protocol=4)),
    )

    # `_global` 行存的是 ProfileManager.meta
    meta_data = dict(
        ver=0,
        updates=True,
        created=1700000000,
        id=42,
        lastMsg=0,
        suppressUpdate=False,
        firstRun=False,
        defaultLang=lang,
    )
    conn.execute(
        "insert or replace into profiles values (?, ?)",
        ("_global", pickle.dumps(meta_data, protocol=4)),
    )
    conn.commit()
    conn.close()
    print(f"init ok: {path} (profile={profile})")


def show(path: str, name: str) -> None:
    conn = sqlite3.connect(path)
    row = conn.execute(
        "select cast(data as blob) from profiles where name = ? collate nocase", (name,)
    ).fetchone()
    conn.close()
    if row is None:
        print(f"missing row: {name}")
        return
    data = pickle.loads(row[0])
    # bytes 无法直接 JSON 化，转成 hex 便于断言「未被破坏」
    printable = {k: (v.hex() if isinstance(v, bytes) else v) for k, v in data.items()}
    print(json.dumps(printable, ensure_ascii=False, indent=2, default=str))


if __name__ == "__main__":
    if len(sys.argv) not in (4, 5):
        raise SystemExit(__doc__)
    action, db_path, name = sys.argv[1], sys.argv[2], sys.argv[3]
    if action == "init":
        # 第 4 个参数是 meta["defaultLang"]（下划线形态，如 zh_CN / en_US）
        init(db_path, name, sys.argv[4] if len(sys.argv) == 5 else "zh_CN")
    elif action == "show":
        show(db_path, name)
    else:
        raise SystemExit(f"unknown action: {action}")
