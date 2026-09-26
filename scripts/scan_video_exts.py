#!/usr/bin/env python3
"""递归扫描某个盘符，统计所有文件扩展名及其数量。

只做统计，不改动任何文件。输出按数量降序，并单独列出「疑似视频」的扩展名。
用法：
    python scripts/scan_video_exts.py D:/ out.json
"""

import json
import os
import sys
import time
from collections import Counter

# 只跳过系统回收站 / 卷信息这类必定无意义的目录
SKIP_DIRS = {"$RECYCLE.BIN", "System Volume Information", "RECYCLER", "lost+found"}

VIDEO_HINT = {
    ".mkv", ".mp4", ".avi", ".rmvb", ".rm", ".mov", ".wmv", ".flv", ".ts", ".m2ts",
    ".mpg", ".mpeg", ".m4v", ".webm", ".vob", ".iso", ".3gp", ".mts", ".m2v",
    ".divx", ".ogm", ".asf", ".f4v", ".mk3d", ".dat", ".tp", ".trp", ".evo",
    ".ogv", ".hdmov", ".mpv", ".m1v", ".qt", ".vp9", ".av1", ".gifv",
}


def main() -> int:
    root = sys.argv[1] if len(sys.argv) > 1 else "D:/"
    out_path = sys.argv[2] if len(sys.argv) > 2 else "scan_exts.json"

    counts: Counter = Counter()
    video_counts: Counter = Counter()
    total = 0
    no_ext = 0
    started = time.time()

    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        for name in filenames:
            total += 1
            _, dot, ext = name.rpartition(".")
            if not dot:
                no_ext += 1
                continue
            ext = "." + ext.lower()
            counts[ext] += 1
            if ext in VIDEO_HINT:
                video_counts[ext] += 1
        if total % 200000 == 0 and total:
            print(f"  ...已扫描 {total} 个文件（{time.time() - started:.0f}s）", flush=True)

    result = {
        "root": root,
        "elapsed_sec": round(time.time() - started, 1),
        "total_files": total,
        "files_without_extension": no_ext,
        "all_extensions": counts.most_common(60),
        "video_extensions": video_counts.most_common(),
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)

    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
