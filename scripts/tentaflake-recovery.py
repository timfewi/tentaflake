#!/usr/bin/env python3
"""Quarantined offline recovery artifact; production checkpoint admission is BLOCKED."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import stat
import tempfile
import time

MAX_BYTES = 1024 * 1024 * 1024
MAX_ENTRIES = 100000
REQUIRED = {
    'pi-durable': ['receipts.sqlite', 'durable.sqlite', 'owner.sqlite'],
    'budget': ['budget.json', 'budget.json.initialized', 'budget.json.lock'],
    'worker': ['.queue.lock', 'pending', 'running', 'approvals', 'jobs', 'reservations', 'results'],
    'control': ['writer.lock', 'receipts.json'],
    'host-api': ['owner.lock', 'actions.sqlite3'],
    'workspace': [],
}


class Refusal(Exception):
    pass


def require(value, message):
    if not value:
        raise Refusal(message)


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(path):
    path = Path(path)
    require(path.is_absolute() and str(path) == os.path.normpath(str(path)), 'noncanonical path')
    for parent in [*reversed(path.parents), path]:
        if parent.exists() or parent.is_symlink():
            require(not parent.is_symlink(), 'symlink ancestry refused')
    return path


def private(path):
    path = canonical(path)
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid()
            and not info.st_mode & 0o077, 'private owned directory required')
    for parent in path.parents:
        info = parent.stat()
        # A sticky temporary parent is usable by the unprivileged synthetic tests.
        require(info.st_uid in (0, os.geteuid()) and
                (not info.st_mode & 0o022 or info.st_mode & stat.S_ISVTX), 'unsafe parent')
    return path


def syncdir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def create(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    syncdir(Path(path).parent)


def load(path, limit=4 * 1024 * 1024):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size <= limit,
                'invalid bounded regular file')
        data = stream.read(limit + 1)
        require(len(data) <= limit, 'file exceeds bound')
        return data


def validate_plan(plan):
    require(set(plan) == {'version', 'coverage', 'scope', 'revision', 'runtime', 'units', 'sources'},
            'invalid checkpoint plan fields')
    require(plan['version'] == 1 and plan['coverage'] == 'selected',
            'whole-system checkpoint unavailable: board service needs an owner-supported checkpoint boundary')
    for field in ('scope', 'revision', 'runtime'):
        require(isinstance(plan[field], str) and re.fullmatch(r'[A-Za-z0-9_.:@/+=-]{1,256}', plan[field]),
                'invalid scope/revision/runtime identity')
    require(isinstance(plan['units'], list) and 0 < len(plan['units']) <= 256
            and len(set(plan['units'])) == len(plan['units']), 'explicit writer units required')
    require(all(isinstance(unit, str) and re.fullmatch(r'[A-Za-z0-9_.@-]+\.(service|path|timer)', unit)
                for unit in plan['units']), 'invalid writer unit')
    require(isinstance(plan['sources'], list) and 0 < len(plan['sources']) <= 256, 'explicit sources required')
    paths = []
    for source in plan['sources']:
        require(set(source) == {'path', 'role', 'schema'} and source['role'] in REQUIRED,
                'unsupported state authority')
        require(type(source['schema']) is int and 0 < source['schema'] <= 65535, 'schema identity required')
        path = canonical(source['path'])
        require(str(path) == source['path'] and path != Path('/'), 'invalid source')
        paths.append(str(path))
    require(len(set(paths)) == len(paths), 'duplicate source')


def tree(root, selected, destination=None):
    """Descriptor-relative no-follow copy, with a digest for every file and directory."""
    entries = {}
    used = 0
    root = canonical(root)
    root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    device = os.fstat(root_fd).st_dev

    def visit(fd, relative):
        nonlocal used
        info = os.fstat(fd)
        name = relative.as_posix()
        require(len(entries) < MAX_ENTRIES, 'entry bound exceeded')
        entries[name] = {'kind': 'directory', 'mode': stat.S_IMODE(info.st_mode), 'uid': info.st_uid, 'gid': info.st_gid}
        if destination is not None and name != '.':
            (destination / relative).mkdir(mode=0o700)
        for child in sorted(os.listdir(fd)):
            rel = relative / child
            child_info = os.stat(child, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISDIR(child_info.st_mode):
                absolute = str(root / rel)
                if absolute in selected and absolute != str(root):
                    continue  # Captured as its own explicit quota/source root.
                require(child_info.st_dev == device, 'unselected nested filesystem')
                child_fd = os.open(child, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                try:
                    visit(child_fd, rel)
                finally:
                    os.close(child_fd)
            else:
                require(stat.S_ISREG(child_info.st_mode) and child_info.st_nlink == 1,
                        'special, linked or symlink state refused')
                child_fd = os.open(child, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
                with os.fdopen(child_fd, 'rb') as stream:
                    before = os.fstat(stream.fileno())
                    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, 'unsafe source changed')
                    used += before.st_size
                    require(used <= MAX_BYTES and len(entries) < MAX_ENTRIES, 'snapshot bound exceeded')
                    hasher = hashlib.sha256()
                    output = contextlib.nullcontext(None)
                    if destination is not None:
                        output = os.fdopen(os.open(destination / rel,
                            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), 'wb')
                    with output as target:
                        total = 0
                        while block := stream.read(1024 * 1024):
                            total += len(block)
                            require(total <= before.st_size, 'source changed during capture')
                            hasher.update(block)
                            if target is not None:
                                target.write(block)
                        if target is not None:
                            target.flush()
                            os.fsync(target.fileno())
                    after = os.fstat(stream.fileno())
                    require((before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                            == (after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
                            and total == before.st_size, 'source changed during capture')
                    entries[rel.as_posix()] = {'kind': 'file', 'size': total, 'sha256': hasher.hexdigest(),
                                             'mode': stat.S_IMODE(before.st_mode), 'uid': before.st_uid, 'gid': before.st_gid}
        if destination is not None:
            syncdir(destination / relative)
    try:
        visit(root_fd, Path('.'))
    finally:
        os.close(root_fd)
    return entries


def receipts(source, entries):
    for name in REQUIRED[source['role']]:
        require(name in entries, 'missing authoritative receipt: ' + name)
    if source['role'] == 'worker':
        require(not any(name.startswith('running/') for name in entries),
                'worker running claims require owner reconciliation; dispatched containers may still write')


def sqlite_evidence(root, entries):
    result = {}
    # Validate disposable copies: opening SQLite can checkpoint/remove WAL, so
    # never open the evidence payload or authoritative source with SQLite.
    for name, entry in entries.items():
        if entry['kind'] != 'file':
            continue
        with open(root / name, 'rb') as stream:
            header = stream.read(16)
        if header != b'SQLite format 3\x00':
            continue
        with tempfile.TemporaryDirectory(prefix='sqlite-check-') as temporary:
            copy = Path(temporary) / 'database'
            shutil.copyfile(root / name, copy)
            for suffix in ('-wal', '-shm', '-journal'):
                if name + suffix in entries:
                    shutil.copyfile(root / (name + suffix), str(copy) + suffix)
            with sqlite3.connect(copy, timeout=0) as db:
                db.set_progress_handler(lambda: int(time.monotonic() > deadline), 1000)
                deadline = time.monotonic() + 10
                require(db.execute('PRAGMA quick_check').fetchall() == [('ok',)], 'SQLite integrity refused')
                result[name] = {'user_version': db.execute('PRAGMA user_version').fetchone()[0],
                                'schema_sha256': digest(encoded(db.execute(
                                    'SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name').fetchall()))}
    return result


def hold(path, binding):
    private(path.parent)
    create(path, encoded({'version': 1, 'reason': 'rollback-uncertainty', 'binding': binding}))


def checkpoint(plan, target, hold_path, boundary):
    validate_plan(plan)
    target = canonical(target)
    private(target.parent)
    require(not target.exists() and not target.is_symlink(), 'checkpoint target must be absent')
    selected = {source['path'] for source in plan['sources']}
    require(not any(target == Path(path) or Path(path) in target.parents for path in selected), 'target inside source')
    binding = digest(encoded(plan))
    hold(hold_path, binding)  # Durable, outside every agent mount; never removed here.
    boundary.stop(plan['units'])
    target.mkdir(mode=0o700)
    syncdir(target.parent)
    try:
        manifests = []
        for index, source in enumerate(plan['sources']):
            directory = target / str(index)
            directory.mkdir(mode=0o700)
            entries = tree(source['path'], selected, directory)
            receipts(source, entries)
            evidence = sqlite_evidence(directory, entries)
            require(entries == tree(source['path'], selected), 'writer changed during checkpoint')
            manifests.append({'source': source, 'entries': entries, 'sqlite': evidence})
        boundary.verify(plan['units'])
        manifest = {'version': 1, 'state': 'quarantined', 'plan': plan, 'plan_sha256': binding,
                    'sources': manifests, 'created_ns': time.time_ns()}
        data = encoded(manifest)
        create(target / 'manifest.json', data)  # Commit record is published last.
        return digest(data)
    except BaseException:
        # Keep private partial evidence for inspection, never label it committed.
        # No hold removal, writer restart, budget refund or uncertain replay.
        raise


def verify(snapshot, expected):
    snapshot = private(snapshot)
    require(re.fullmatch('[0-9a-f]{64}', expected) is not None, 'external manifest digest required')
    data = load(snapshot / 'manifest.json')
    require(digest(data) == expected, 'manifest digest mismatch')
    manifest = json.loads(data)
    require(set(manifest) == {'version', 'state', 'plan', 'plan_sha256', 'sources', 'created_ns'}
            and manifest['version'] == 1 and manifest['state'] == 'quarantined', 'invalid checkpoint manifest')
    validate_plan(manifest['plan'])
    require(manifest['plan_sha256'] == digest(encoded(manifest['plan'])), 'plan binding mismatch')
    require(len(manifest['sources']) == len(manifest['plan']['sources']), 'source count mismatch')
    require(set(os.listdir(snapshot)) == {'manifest.json', *(str(i) for i in range(len(manifest['sources'])))},
            'unexpected checkpoint content')
    for index, source in enumerate(manifest['sources']):
        require(set(source) == {'source', 'entries', 'sqlite'}
                and source['source'] == manifest['plan']['sources'][index], 'source identity mismatch')
        actual = tree(snapshot / str(index), set())
        # Snapshot ownership is deliberately private, not the original service UID.
        def content(entries):
            return {name: {key: value for key, value in entry.items() if key not in ('uid', 'gid', 'mode')}
                    for name, entry in entries.items()}
        require(content(actual) == content(source['entries']), 'payload digest or receipt mismatch')
        receipts(source['source'], actual)
        require(sqlite_evidence(snapshot / str(index), actual) == source['sqlite'], 'SQLite identity mismatch')
    return manifest


def restore(snapshot, target, expected):
    manifest = verify(snapshot, expected)
    target = canonical(target)
    private(target.parent)
    require(not target.exists() and not target.is_symlink(), 'restore target must be new and absent')
    require(not any(target == Path(s['path']) or Path(s['path']) in target.parents
                    for s in manifest['plan']['sources']), 'restore cannot target live state')
    target.mkdir(mode=0o700)
    syncdir(target.parent)
    hold(target / 'recovery-hold', expected)
    # Deliberately no activation/reconcile command: even an old coherent snapshot
    # cannot authorize lower spend, new IDs, forgotten dispatch or cleared holds.
    for index, source in enumerate(manifest['sources']):
        destination = target / str(index)
        destination.mkdir(mode=0o700)
        tree(Path(snapshot) / str(index), set(), destination)
    create(target / 'manifest.json', encoded(manifest))
    # Verify copied bytes again; marker stays even if this fails/interruption occurs.
    for index, source in enumerate(manifest['sources']):
        copied = tree(target / str(index), set())
        original = tree(Path(snapshot) / str(index), set())
        require(copied == original, 'restore changed during copy')
    create(target / 'restore-receipt.json', encoded({'version': 1, 'manifest_sha256': expected,
           'state': 'quarantined', 'activation': 'forbidden', 'budget_rollback': 'forbidden',
           'operation_replay': 'forbidden'}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    capture = sub.add_parser('checkpoint')
    capture.add_argument('--plan', required=True)
    capture.add_argument('--target', required=True)
    recovery = sub.add_parser('restore')
    recovery.add_argument('--snapshot', required=True)
    recovery.add_argument('--target', required=True)
    recovery.add_argument('--digest', required=True)
    args = parser.parse_args()
    os.umask(0o077)
    def expired(_signum, _frame):
        raise Refusal('bounded recovery deadline expired; holds and private partial evidence retained')
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(300)
    if args.command == 'checkpoint':
        raise SystemExit(
            'BLOCKED: production checkpoint admission is NOT IMPLEMENTED. '
            'Runtime, broker, worker and control lifecycle hold enforcement and '
            'canonical writer coverage are required; board checkpoint boundary is unavailable.')
    else:
        restore(args.snapshot, args.target, args.digest)


if __name__ == '__main__':
    try:
        main()
    except (Refusal, OSError, ValueError, sqlite3.Error):
        raise SystemExit('Recovery refused; retain holds and private partial evidence. No activation authorized.')
