#!/usr/bin/env python3
"""诊断「切号时同步失败」：一条命令拿到为什么同步不过去的事实，直接贴回 issue。

用法: python scripts/diagnostics/diagnose-sync-failure.py [标题关键词 ...]
      [--home <目录>]（造测试 fixture 用）[--json]（机器可读结果）

严格只读：文件按只读打开，数据库用 `mode=ro` 连接，不写、不修、不备份。
为什么需要它：客户端只说一句「同步失败」，真实原因却可能相反——正文末行半截、成员行缺失、
同名正文副本、预览过期、缺少共同基线，或上一次写盘没走完（同一目标上有未完成操作时执行
阶段会直接拒绝）。这里把这些事实摊开并给一组近似判定。
脱敏：不打印任何正文内容；uid / accountId 只留前 6 位；不打印 token 类字段。
"""
import argparse
import datetime
import glob
import hashlib
import json
import os
import sqlite3
import struct
import sys

# 与 Rust 侧一致的口径（改这里必须同步 content.rs / link_store.rs / baseline.rs / operation_log.rs）。
MARK = "__wb_shift_session_id__"
LINK_STORE_VERSION = 1
BASELINE_VERSION = 1
NORMALIZATION_VERSION = 1
LINES_SALT = b"workbuddy-shift-lines-v1\0"
# 终止态：完成与「什么都没写就放弃」都不算未完成（同 OpPhase::is_unfinished）。
CLOSED_PHASES = ("completed", "abandoned")
SESSION_COLUMNS = ("id", "user_id", "title", "custom_title", "status",
                   "last_activity_at", "updated_at", "deleted_at")
TAIL_LABELS = {"ok": "正常", "truncated": "末行半截（EOF）", "glued": "一行粘连多个 JSON",
               "invalid": "非法 JSON", "empty": "存在空行", "nul": "含裸 NUL 字节",
               "missing": "正文缺失"}

def parse_args(argv):
    parser = argparse.ArgumentParser(description="诊断切号时同步失败的原因（只读）")
    parser.add_argument("keywords", nargs="*", help="按标题关键词筛选（不传则诊断全部组）")
    parser.add_argument("--home", default=None, help="覆盖家目录（造测试 fixture 用）")
    parser.add_argument("--json", action="store_true", dest="as_json", help="输出机器可读结果")
    return parser.parse_args(argv)

def clip(value, keep):
    """id 类字段截断：够定位到是谁，又不至于把完整标识扩散到 issue 里。"""
    text = "" if value is None else str(value)
    return text[:keep] + "…" if len(text) > keep else (text or "-")

def format_ms(value):
    if not isinstance(value, int) or value <= 0:
        return "-"
    try:
        seconds = value / 1000.0 if value > 10**12 else float(value)
        return datetime.datetime.fromtimestamp(seconds).strftime("%Y-%m-%d %H:%M")
    except (OverflowError, OSError, ValueError):
        return str(value)

def line_digest(chunk, session_id):
    """单行摘要 = SHA-256(长度前缀 + 原文)，只把本侧 sessionId 换成固定标记。

    与 `content.rs::line_digest_of` 同口径。必须换掉 sessionId：不做这一步任何健康副本的
    逐行摘要都会**全部**不同，工具会把「已经同步好的组」报成分歧——比不判还糟。
    """
    marked = chunk.replace(session_id.encode(), MARK.encode()) if session_id else chunk
    return hashlib.sha256(struct.pack(">Q", len(marked)) + marked).hexdigest()

def inspect_line(text, number, is_last, has_nul):
    """单行判定 + 说明；文案骨架与 `content.rs::describe_parse_failure` 同义。"""
    decoder, index, count = json.JSONDecoder(), 0, 0
    while index < len(text):
        while index < len(text) and text[index] in " \t\r":
            index += 1
        if index >= len(text):
            break
        try:
            _, index = decoder.raw_decode(text, index)
        except ValueError:
            break
        count += 1
    complete = index >= len(text)
    kind = "ok" if complete and count == 1 else ("glued" if complete and count else "invalid")
    error = None
    if kind == "invalid":
        try:
            json.loads(text)
        except json.JSONDecodeError as thrown:
            error = thrown
    if has_nul:
        kind = "nul"
    elif kind == "invalid" and is_last and error is not None and (
            # 半截行 = 字符串未闭合，或解析恰好在末尾断开（进程被杀留下的末行）。
            error.msg.startswith("Unterminated string") or error.pos >= len(text) - 1):
        kind = "truncated"
    if kind == "ok":
        return "ok", ""
    tail, size = ("文件末行" if is_last else "不是末行"), len(text)
    if kind == "glued":
        return kind, f"第 {number} 行粘连了多个 JSON 对象（该行 {size} 字节；{tail}），内容可能不完整"
    if kind == "nul":
        return kind, f"第 {number} 行含裸 NUL 字节（该行 {size} 字节；{tail}），无法确认"
    if kind == "truncated":
        return kind, f"第 {number} 行的格式无法识别（该行 {size} 字节；{tail}，JSON 未闭合），内容可能不完整"
    detail = "解析器接受了该行" if error is None else error.msg
    column = 0 if error is None else error.colno
    return kind, f"第 {number} 行的格式无法识别（第 {column} 列，{detail}；该行 {size} 字节；{tail}），内容可能不完整"

def scan_body(data_root, session_id):
    """读一个成员的正文并给出健康度；只读，结果里**不含**任何正文内容。"""
    body = {"missing": True, "path": None, "bytes": 0, "records": 0, "digests": [],
            "parseable": False, "tail": "missing", "tailLine": None, "problems": [],
            "problemCount": 0, "endsWithNewline": False, "lastAssistant": None,
            "reason": "正文文件不存在（projects/*/<sessionId>.jsonl 都没找到）"}
    # 同一 sessionId 可能在多个 projects/<workspace>/ 下都有正文：Rust 侧按 read_dir 顺序取，
    # 与这里的排序可能不同，读到不同文件本身就是同步失败的成因之一，因此全部列出。
    matches = sorted(glob.glob(os.path.join(data_root, "projects", "*", session_id + ".jsonl")))
    if not matches:
        return body
    try:
        with open(matches[0], "rb") as handle:  # 只读：不写、不修
            raw = handle.read()
    except OSError as error:
        # 同名路径是目录、权限不足、被占用…：记录现场但别让整个诊断崩掉。
        body.update({"path": os.path.relpath(matches[0], data_root).replace("\\", "/"),
                     "copies": [os.path.relpath(path, data_root).replace("\\", "/")
                                for path in matches],
                     "reason": f"正文无法读取：{error}"})
        return body
    lines = raw.split(b"\n")
    while lines and lines[-1] == b"":  # 末尾换行（含多个）不算记录，同 normalize_jsonl
        lines.pop()
    problems, digests, assistant, records, last = [], [], None, 0, 0
    for number, chunk in enumerate(lines, start=1):
        chunk = chunk[:-1] if chunk.endswith(b"\r") else chunk
        if not chunk.strip():
            problems.append({"line": number, "kind": "empty", "detail": f"第 {number} 行为空，内容可能不完整"})
            continue
        records, last = records + 1, number
        text = chunk.decode("utf-8", "replace")
        kind, detail = inspect_line(text, number, number == len(lines), b"\x00" in chunk)
        if kind != "ok":
            problems.append({"line": number, "kind": kind, "detail": detail})
        digests.append(line_digest(chunk, session_id))
        if kind == "ok" and '"assistant"' in text:
            record = json.loads(text)
            if isinstance(record, dict) and record.get("role") == "assistant" \
                    and record.get("type") == "message":
                provider = record.get("providerData")
                error = provider.get("error") if isinstance(provider, dict) else None
                assistant = {"status": record.get("status"), "hasError": bool(error),
                             "errorKeys": sorted(error) if isinstance(error, dict) else []}
    body.update({
        "missing": False, "bytes": len(raw), "records": records, "digests": digests,
        "parseable": not problems, "problems": problems, "tail": "ok",
        "problemCount": len(problems), "endsWithNewline": raw.endswith(b"\n"),
        "lastAssistant": assistant,
        # 只显示 `~/.workbuddy` 之后的部分，避免把用户名带进 issue。
        "path": os.path.relpath(matches[0], data_root).replace("\\", "/"),
        "copies": [os.path.relpath(path, data_root).replace("\\", "/") for path in matches],
        # Rust 侧读到第一条坏行就中止，因此「原因」取第一条。
        "reason": problems[0]["detail"] if problems else ""})
    if records == 0:
        body.update({"tail": "empty", "reason": "内容为空"})
        return body
    body["tailLine"] = last
    body["tail"] = next((p["kind"] for p in problems if p["line"] == last), "ok")
    return body

def read_session_rows(db_path, session_ids):
    """逐列探测后读 sessions 行；返回 (行字典, 错误说明)。缺列只影响显示，不算失败。"""
    if not os.path.exists(db_path):
        return {}, f"数据库不存在：{db_path}"
    try:
        # `mode=ro` 保证连接本身不会产生 -wal / -shm 写入。
        uri = f"file:{os.path.abspath(db_path).replace(chr(92), '/')}?mode=ro&immutable=0"
        connection = sqlite3.connect(uri, uri=True)
        connection.row_factory = sqlite3.Row
    except sqlite3.Error as error:
        return {}, f"数据库无法只读打开：{error}"
    try:
        columns = [row["name"] for row in connection.execute("PRAGMA table_info(sessions)")]
        if not columns:
            return {}, "sessions 表不存在"
        sql = f"SELECT {', '.join(n for n in SESSION_COLUMNS if n in columns)} FROM sessions WHERE id = ?"
        rows = {}
        for session_id in session_ids:
            row = connection.execute(sql, (session_id,)).fetchone()
            rows[session_id] = dict(row) if row is not None else None
        return rows, None
    except sqlite3.Error as error:
        return {}, f"读取 sessions 表失败：{error}"
    finally:
        connection.close()

def load_pair_baseline(store_root, group, member_a, member_b):
    """配对基线状态，与 `BaselineState` 同口径（ready / missing / unverifiable）。"""
    broken = {"digests": None, "state": "unverifiable", "reason": "上次同步的记录缺失或已损坏"}
    wanted, ref, version = sorted([member_a, member_b]), None, None
    for candidate in only_dicts(group.get("pairBases")):
        ids = candidate.get("memberIds")
        if not isinstance(ids, list):
            continue
        if sorted(str(item) for item in ids) == wanted:
            version = candidate.get("normalizationVersion")
            ref = str(candidate.get("baselineRef") or "")
            break
    if ref is None:
        return {"digests": None, "state": "missing", "reason": "找不到双方上次一致的内容，暂时无法同步"}
    if version != NORMALIZATION_VERSION:
        return dict(broken, reason=f"记录格式版本 {version} 不受支持（当前 {NORMALIZATION_VERSION}）")
    try:
        with open(os.path.join(str(store_root), "session-links", "baselines", ref + ".json"),
                  encoding="utf-8") as handle:
            record = json.load(handle)
    except (OSError, ValueError):
        return broken
    digests = record.get("lineDigests") or []
    # 只校版本与记录数（不重算总摘要）：坏基线必须当成「不可验证」，不能当成「可用」。
    if (record.get("version") != BASELINE_VERSION or record.get("recordCount") != len(digests)
            or record.get("normalizationVersion") != NORMALIZATION_VERSION):
        return broken
    return {"digests": digests, "state": "ready", "reason": ""}

def approx_decision(source, target, baseline):
    """按 `sync_decision.rs::decide_sync` 的顺序做只读推断，返回 (verdict, 原因)。

    刻意不实现「末条 incomplete 归一」的折算：漏了只会让结论偏保守（把一致说成分歧），
    因此一律标注为近似判定。
    """
    if source["missing"]:
        return "unknown", "当前账号的内容不存在，无法确认"
    if not source["parseable"]:
        return "unknown", "当前账号的内容无法确认：" + source["reason"]
    if target["missing"]:
        return "unknown", "目标账号的内容不存在，无法确认"
    if not target["parseable"]:
        return "unknown", "目标账号的内容无法确认：" + target["reason"]
    theirs, mine = source["digests"], target["digests"]

    def prefix_of(shorter, longer):
        """严格有序前缀（判定里 fastForward / ahead 的前提之一）。"""
        return len(shorter) < len(longer) and longer[: len(shorter)] == shorter

    if theirs == mine:
        return "identical", f"两边的内容一致（各 {source['records']} 条），不需要同步"
    if prefix_of(mine, theirs):
        added = source["records"] - target["records"]
        return "fastForward", f"目标账号没有独有改动，来源账号新增 {added} 条，可以直接同步"
    extra = target["records"] - source["records"]
    ahead = f"目标账号已包含当前账号的全部内容，另多出 {extra} 条，本次不会同步过去"
    if baseline["state"] != "ready":
        return ("ahead", ahead) if prefix_of(theirs, mine) else ("unknown", baseline["reason"])
    if theirs == baseline["digests"] or (prefix_of(theirs, mine) and mine != baseline["digests"]):
        return "ahead", ahead
    return "diverge", f"两边都有改动（目标账号独有的 {extra} 条会被替换）；覆盖会替换目标账号的完整内容"

def only_dicts(value):
    """磁盘上的 JSON 形状不可信：不是数组返回空，数组里非字典的元素直接丢掉。"""
    return [item for item in value if isinstance(item, dict)] if isinstance(value, list) else []

def load_links(links_path):
    """读关联存储；缺失 / 版本不符 / 形状不对都给可读提示，而不是 traceback。"""
    if not os.path.exists(links_path):
        return None, (f"同步记录不存在：{links_path}\n  → 首次使用、或还没建立任何「关联会话」时"
                      "就是这种状态；此时客户端里也不会出现可同步的组。")
    try:
        with open(links_path, encoding="utf-8") as handle:
            store = json.load(handle)
    except (OSError, ValueError) as error:
        return None, f"同步记录无法读取或已损坏：{links_path}（{error}）"
    # 合法 JSON 但不是对象（`null` / `[]` / 字符串）同样是坏文件：不能拿它当字典用。
    if not isinstance(store, dict):
        return None, f"同步记录结构异常（顶层不是 JSON 对象）：{links_path}"
    if store.get("version") != LINK_STORE_VERSION:
        return None, (f"同步记录版本 {store.get('version')} 不受支持"
                      f"（当前支持 {LINK_STORE_VERSION}）：{links_path}")
    return store, None

def scan_pending_operations(store_root):
    """未完成的操作日志：写盘阶段失败最直接的证据（只读，按 updatedAt 倒序，最多 10 条）。

    `execute_sync_item` 发现同一目标上有未完成操作时会直接拒绝整项，真正的原因只存在这条
    记录的 `lastError` 里。坏文件不能崩掉整个诊断：目录缺失 / JSON 损坏 / 字段缺失一律跳过。
    """
    pending = []
    try:
        root = str(store_root)
        for path in glob.glob(os.path.join(root, "session-links", "operations", "*.json")):
            try:
                with open(path, encoding="utf-8") as handle:
                    record = json.load(handle)
            except (OSError, ValueError):
                continue
            if not isinstance(record, dict) or record.get("phase") in CLOSED_PHASES:
                continue
            operation_id = str(record.get("operationId") or os.path.basename(path)[:-5])
            target = record.get("target")
            pending.append({
                "operationId": operation_id, "kind": record.get("kind"),
                "phase": record.get("phase"), "groupId": record.get("groupId"),
                "targetSessionId": target.get("sessionId") if isinstance(target, dict) else None,
                "expectedRecordCount": record.get("expectedRecordCount"),
                "updatedAt": record.get("updatedAt"), "lastError": record.get("lastError") or "",
                # 现场备份只判存在性：`backups/<opId>`（另有记录里清单文件这条线索）。
                "hasBackup": os.path.isdir(os.path.join(root, "backups", operation_id))
                or os.path.exists(str(record.get("backup") or ""))})
    except (OSError, ValueError, TypeError, KeyError):
        return []
    pending.sort(key=lambda item: item.get("updatedAt") or 0, reverse=True)
    return pending[:10]

def member_report(data_root, member, rows):
    """单个成员的事实：DB 行 + 正文健康度；`bodyCopies` 是同一 sessionId 的全部正文路径。"""
    session_id = str(member.get("sessionId") or "")
    row, body = rows.get(session_id) or {}, scan_body(data_root, session_id)
    return {"memberId": member.get("memberId"), "uid": member.get("uid"),
            "accountId": member.get("accountId"), "sessionId": session_id,
            "state": member.get("state"), "lastSyncedAt": member.get("lastSyncedAt"),
            "db": {"exists": bool(row), "userId": row.get("user_id"),
                   "ownerMismatch": bool(row) and row.get("user_id") not in (None, member.get("uid")),
                   "softDeleted": row.get("deleted_at") not in (None, 0, ""),
                   "status": row.get("status"), "lastActivityAt": row.get("last_activity_at"),
                   "updatedAt": row.get("updated_at")},
            "bodyCopies": body.pop("copies", []), "body": body}

def render_member(index, member):
    """成员块（不打印任何正文内容）。"""
    db, body, copies = member["db"], member["body"], member["bodyCopies"]
    out = [f"  成员[{index}] uid={clip(member['uid'], 6)} "
           f"accountId={clip(member['accountId'], 6)} state={member['state']} "
           f"sessionId={clip(member['sessionId'], 8)}"]
    if not db["exists"]:
        out.append("    DB 行：不存在（客户端里这条会话可能已被清理）")
    else:
        flags = [n for k, n in (("softDeleted", "已软删除"), ("ownerMismatch", "归属异常")) if db[k]]
        out.append(f"    DB 行：存在{'（' + '、'.join(flags) + '）' if flags else ''} "
                   f"status={clip(db['status'], 99)} last_activity_at="
                   f"{format_ms(db['lastActivityAt'])} updated_at={format_ms(db['updatedAt'])}")
    if body["path"] is None:
        return out + ["    正文：未找到", "    最后一条 assistant：未找到"]
    out.append(f"    正文：{body['path']}（{body['bytes']:,} 字节 / {body['records']:,} 条记录）")
    if len(copies) > 1:
        out.append(f"    ⚠ 同一会话存在 {len(copies)} 份正文，工具实际读取哪一份不确定，"
                   "可能是本条同步失败的成因：")
        out.extend("      · " + copy for copy in copies)
    if body["tail"] != "ok":
        position = f"第 {body['tailLine']} 行" if body["tailLine"] else "全文"
        note = (f"；{body['reason']}" if body["tail"] == "missing"
                else "；文件未以换行结尾" if not body["endsWithNewline"] else "")
        out.append(f"    末行：{TAIL_LABELS.get(body['tail'], body['tail'])}（{position}{note}）")
    elif body["problemCount"]:
        # 末行完好但中途有坏行时也要点出来，否则「末行：正常」会让人以为正文没问题。
        out.append(f"    末行：正常，但正文另有 {body['problemCount']} 处异常（见下）")
    else:
        out.append("    末行：正常")
    out.extend("      · " + problem["detail"] for problem in body["problems"][:3])
    assistant = body["lastAssistant"]
    if assistant is None:
        out.append("    最后一条 assistant：未找到")
    else:
        error = ("带 providerData.error（键：{}）".format("、".join(assistant["errorKeys"]) or "未知")
                 if assistant["hasError"] else "不带 providerData.error")
        out.append(f"    最后一条 assistant：status={clip(assistant['status'], 99)}，{error}")
    return out

def render_pending(pending):
    """未完成操作节：`== 未完成的会话操作 ==`，一条记录一行（lastError 全文，不换行）。"""
    out = ["", "== 未完成的会话操作 =="]
    if not pending:
        return out + ["  没有未完成的会话操作记录"]
    for item in pending:
        count = "-" if item["expectedRecordCount"] is None else item["expectedRecordCount"]
        # 空白折叠成单个空格：让整条记录留在一行，同时**不丢** lastError 任何字符。
        error = " ".join(str(item["lastError"]).split()) or "（无 lastError，可能停在进程被杀的中途）"
        out.append(f"  · kind={clip(item['kind'], 99)} phase={clip(item['phase'], 99)} "
                   f"group={clip(item['groupId'], 8)} 目标会话={clip(item['targetSessionId'], 8)} "
                   f"预期记录数={count} 更新于={format_ms(item['updatedAt'])} 现场备份="
                   f"{'有' if item['hasBackup'] else '无'} lastError={error}")
    return out

def suggestions(groups, pending):
    """下一步建议：只指向可执行的事实，不承诺任何自动修复。"""
    tips = []
    if pending:
        tips.append(f"存在 {len(pending)} 条未完成的会话操作记录：同一目标上有未完成操作时同步会被"
                    "直接拒绝；下次切号时客户端会尝试恢复，长期停在同一 phase 就把上面这段贴回 issue。")
    for group in groups:
        for side in group["members"]:
            body, db, name = side["body"], side["db"], clip(group["groupId"], 8)
            if body["tail"] in ("truncated", "glued", "invalid", "nul", "empty"):
                tips.append(
                    f"组 {name} 的一侧正文不完整（{TAIL_LABELS.get(body['tail'], body['tail'])}，"
                    f"第 {body['tailLine']} 行）。可以先看 "
                    "`python scripts/diagnostics/fix-incomplete-turns.py`：它只处理「末条 "
                    "assistant 停在 incomplete」，**不修坏行**（不提供自动改正文的 --repair）；"
                    "坏行建议重开客户端让它自己回写，动手前先备份。")
            if len(side["bodyCopies"]) > 1:
                tips.append(f"组 {name} 的一侧有 {len(side['bodyCopies'])} 份同名正文：先确认哪一份"
                            "是当前在用的（客户端最近改过的那份），否则同步可能读写两份不同的文件。")
            if not db["exists"] or db["softDeleted"] or db["ownerMismatch"]:
                tips.append(f"组 {name} 的一侧会话行缺失或归属异常：在 App 里解除该会话的关联，"
                            "再重新「新增关联会话」（重新登记后两侧才是同一条逻辑会话）。")
    return tips or ["没有发现正文或会话行的结构性异常。若客户端仍提示同步失败，请把本次输出"
                    "与客户端的具体提示一起贴回 issue——近似判定与客户端显示不一致本身就是线索。"]

def build_report(args):
    home = os.path.expanduser(args.home) if args.home else os.path.expanduser("~")
    store_root, data_root = os.path.join(home, ".workbuddy-shift"), os.path.join(home, ".workbuddy")
    links_path = os.path.join(store_root, "session_links.json")
    report = {
        "home": home, "linksPath": links_path, "dataRoot": data_root, "storeError": None,
        "dbPath": os.path.join(data_root, "workbuddy.db"), "dbError": None,
        "groups": [], "suggestions": [], "pendingOperations": scan_pending_operations(store_root),
        "notes": ["判定是近似推断，以客户端显示为准（不跑客户端的末条归一折算）；"
                  "「当前账号 / 目标账号」按组成员顺序给出，不代表客户端里的切换方向。",
                  "行摘要 = SHA-256(长度前缀 + 原文)，只把本侧 sessionId 换成固定标记；两侧 "
                  "sessionId 必然不同，不做这一步归一任何健康副本都会被判成分歧。",
                  "全程只读：未修改任何正文、数据库或同步记录，也未打印正文内容。"]}
    store, report["storeError"] = load_links(links_path)
    # 关联表是磁盘上的文件，形状不可信：groups / members / pairBases 被写成字符串时
    # 逐字符迭代进去只会抛 AttributeError，于是逐一过滤成「只处理字典元素」。
    groups = only_dicts((store or {}).get("groups"))
    session_ids = sorted({
        str(member.get("sessionId") or "")
        for group in groups for member in group.get("members") or []
        if isinstance(member, dict)})
    rows, report["dbError"] = read_session_rows(
        report["dbPath"], session_ids)
    keywords = [word.strip() for word in args.keywords if word.strip()]
    for group in groups:
        raw_members = only_dicts(group.get("members"))
        members = [item for item in raw_members if item.get("state") == "active"]
        if len(members) < 2:
            members = raw_members[:2]
        titles = []
        for member in members:
            row = rows.get(str(member.get("sessionId") or "")) or {}
            title = row.get("custom_title") or row.get("title") or ""
            # 同一逻辑会话的各账号副本标题通常相同，重复显示只会占满一行。
            if title and title not in titles:
                titles.append(title)
        if keywords and not any(word in title for word in keywords for title in titles):
            continue
        baseline = {"digests": None, "state": "missing", "reason": ""}
        if len(members) >= 2:
            baseline = load_pair_baseline(store_root, group, str(members[0].get("memberId") or ""),
                                          str(members[1].get("memberId") or ""))
        items = [member_report(data_root, member, rows) for member in members]
        decision = None
        if len(items) >= 2:
            verdict, reason = approx_decision(items[0]["body"], items[1]["body"], baseline)
            decision = {"verdict": verdict, "reason": reason}
        report["groups"].append({
            "groupId": str(group.get("id") or ""), "variant": group.get("variant"),
            "titles": titles, "members": items, "decision": decision,
            "baselineState": baseline["state"], "baselineReason": baseline["reason"]})
    report["suggestions"] = suggestions(report["groups"], report["pendingOperations"])
    return report

def render_text(report):
    out = [f"WorkBuddy Shift 同步失败诊断（只读）\n"
           f"家目录：{report['home']}\n同步记录：{report['linksPath']}\n"
           f"数据根：{report['dataRoot']}"]
    if report["dbError"]:
        out.append(f"数据库提示：{report['dbError']}（DB 行状态将全部显示为不存在）")
    if report["storeError"]:
        out.append(f"\n无法继续诊断关联组：{report['storeError']}")
    out.append(f"\n组数：{len(report['groups'])}")
    for index, group in enumerate(report["groups"], start=1):
        title = "／".join(group["titles"]) if group["titles"] else "(无标题)"
        out.extend(["", "=" * 72, f"组 {index}. {clip(group['groupId'], 8)}  "
                                  f"variant={group['variant']}  「{title}」"])
        for member_index, member in enumerate(group["members"], start=1):
            out.extend(render_member(member_index, member))
        decision = group["decision"] or {"verdict": "无法给出", "reason": "该组有效成员不足两个"}
        out.append(f"  近似判定：{decision['verdict']} —— {decision['reason']}")
        reason = f"（{group['baselineReason']}）" if group["baselineReason"] else ""
        out.append(f"  配对基线：{group['baselineState']}{reason}")
        out.append("  ⚠ 近似判定，以客户端显示为准")
    # 未完成操作放在各组判定之后：先看清两侧事实，再看「是不是上次写盘根本没走完」。
    out.extend(render_pending(report["pendingOperations"]))
    out.extend(["", "=" * 72, "下一步建议："])
    out.extend("  - " + tip for tip in report["suggestions"])
    out.append("\n说明：")
    out.extend("  - " + note for note in report["notes"])
    print("\n".join(out))

def main(argv):
    args = parse_args(argv)
    report = build_report(args)
    if args.as_json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        render_text(report)
    # 只读诊断：发现问题是结果，不是脚本失败；只有读不到同步记录才算「没跑成」。
    return 1 if report["storeError"] else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
