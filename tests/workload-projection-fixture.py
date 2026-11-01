"""Synthetic NIC-less VM fixture; never consumes host/user configuration or runs.

Only the test orchestrator is root. Queries exec as exact non-root Unix users;
Hostpulse runs through its unchanged hardened system module. All output is a
stage label or sanitized assertion, not unrelated metrics or configuration.
"""

import hashlib
import json
import math
import os
from pathlib import Path
import pwd
import socket
import struct
import subprocess
import sys
import time

POLICY = Path('/run/workload-projection-policy/config.toml')
FACTS = Path('/run/tentaflake-workloads')
SOCKET = '/run/hostpulse-projection/metadata.sock'
FIELDS = {'captured_at_ms', 'state', 'cpu_usage_us_per_second',
          'memory_current_bytes', 'memory_max_bytes',
          'io_read_bytes_per_second', 'io_write_bytes_per_second'}


def provision():
    POLICY.parent.mkdir(mode=0o755, exist_ok=True)
    os.chmod(POLICY.parent, 0o755)
    text = '''version = 1
[storage]
retention_days = 1
max_size = "16 MiB"
[system]
enabled = false
[services]
enabled = false
[journal]
enabled = false
[agents]
enabled = false
'''
    for name, uid in [('one', 2100), ('two', 2101)]:
        text += f'''
[[resources.bindings]]
tenant_id = "tenant-{name}"
agent_id = "agent-{name}"
workload_id = "workload-{name}"
policy_revision = "fixture-1"
manifest_file = "/run/tentaflake-workloads/agent-{name}.json"
[[resources.readers]]
uid = {uid}
tenant_id = "tenant-{name}"
agent_id = "agent-{name}"
'''
    POLICY.write_text(text)
    os.chmod(POLICY, 0o644)


def command(*args, timeout=25, user=None, success=True):
    credentials = {}
    if user is not None:
        account = pwd.getpwnam(user)
        credentials = dict(user=account.pw_uid, group=account.pw_gid, extra_groups=[])
    result = subprocess.run(args, capture_output=True, text=True,
                            timeout=timeout, check=False, **credentials)
    assert (result.returncode == 0) == success, f'command failed: {args[0:2]}'
    return result.stdout.strip()


def service(action, *units):
    command('systemctl', action, *units)


def eventually(label, operation, timeout=25):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            result = operation()
            if result:
                return result
        except (FileNotFoundError, json.JSONDecodeError, AssertionError):
            pass
        time.sleep(0.25)
    raise AssertionError('deadline: ' + label)


def fact(name):
    return json.loads((FACTS / f'agent-{name}.json').read_text())


def live(name):
    return eventually('live ' + name, lambda: fact(name) if fact(name)['state'] == 'live' else None)


def exact(stream, count):
    data = bytearray()
    while len(data) < count:
        part = stream.recv(count - len(data))
        if not part:
            raise ConnectionError('closed projection')
        data.extend(part)
    return data


def query(name):
    request = json.dumps(dict(version=1, operation='resources',
                              tenant_id='tenant-' + name, agent_id='agent-' + name,
                              limit=120)).encode()
    try:
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(2)
            stream.connect(SOCKET)
            stream.sendall(struct.pack('>I', len(request)) + request)
            size = struct.unpack('>I', exact(stream, 4))[0]
            assert 0 < size <= 256 * 1024, 'projection frame bound'
            return json.loads(exact(stream, size))
    except (ConnectionError, PermissionError):
        return {'closed': True}


def read(name, user=None):
    return json.loads(command(sys.executable, __file__, 'query', name,
                              user=user or 'reader-' + name, timeout=5))


def available(name):
    def attempt():
        value = read(name)
        return value if value.get('status') == 'ok' and len(value['samples']) >= 2 else None
    return eventually('resource samples ' + name, attempt)


def unavailable(name):
    value = read(name)
    assert value['status'] == 'unavailable', 'provenance loss must fail closed'
    assert value['resource'] is None and value['samples'] == [] and value['gap']


def counters(name):
    current = fact(name)
    assert current['state'] == 'live'
    pid = current['pid']
    assert Path(f'/proc/{pid}/cgroup').read_text().strip() == '0::' + current['cgroup']
    stat = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    assert int(stat[19]) == current['start_ticks'], 'kernel process start binding'
    group = Path('/sys/fs/cgroup' + current['cgroup'])
    assert group.stat().st_ino == current['cgroup_inode'], 'kernel inode binding'
    cpu = dict(line.split() for line in (group / 'cpu.stat').read_text().splitlines())
    io = {}
    for line in (group / 'io.stat').read_text().splitlines():
        fields = line.split()
        row = dict(field.split('=') for field in fields[1:])
        io[fields[0]] = (int(row['rbytes']), int(row['wbytes']))
    return dict(time=time.time_ns() // 1_000_000, cpu=int(cpu['usage_usec']),
                memory=int((group / 'memory.current').read_text()),
                maximum=int((group / 'memory.max').read_text()), io=io)


def validate(name, value):
    assert set(value) == {'version', 'status', 'resource', 'samples', 'gap', 'stale'}
    assert value['version'] == 1 and value['status'] == 'ok' and not value['stale']
    assert value['resource'] == dict(tenant_id='tenant-' + name,
                                    agent_id='agent-' + name,
                                    workload_id='workload-' + name,
                                    generation=fact(name)['generation'])
    assert 1 <= len(value['samples']) <= 120
    previous = 0
    for row in value['samples']:
        assert set(row) == FIELDS, 'no raw events, paths or run attribution'
        assert row['captured_at_ms'] > previous
        previous = row['captured_at_ms']
        assert row['state'] in ('available', 'partial')
        for key in FIELDS - {'captured_at_ms', 'state'}:
            number = row[key]
            assert number is None or (type(number) in (int, float)
                                      and math.isfinite(number) and 0 <= number <= 2**53 - 1)


def first(name):
    value = available(name)
    validate(name, value)
    assert value['gap'], 'history reset must disclose gap'
    row = value['samples'][0]
    assert row['cpu_usage_us_per_second'] is None
    assert row['io_read_bytes_per_second'] is None
    assert row['io_write_bytes_per_second'] is None
    return value


def load():
    # Fixed work and bounded sleeps; functional smoke only, never a benchmark.
    block = b'SYNTHETIC' * 131072
    with open('/synthetic-io', 'wb', buffering=0) as output:
        for _ in range(12):
            output.write(block)
            os.fsync(output.fileno())
            for _ in range(8):
                hashlib.sha256(block).digest()
            time.sleep(0.5)
    with open('/synthetic-io', 'rb', buffering=0) as source:
        for _ in range(12):
            source.read(len(block))


def workload():
    memory = bytearray(int(os.environ['FIXTURE_MIB']) * 1024 * 1024)
    for offset in range(0, len(memory), 4096):
        memory[offset] = 1
    # Finite lifetime even if test driver disappears; systemd teardown is earlier.
    time.sleep(450)


def test():
    print('synthetic projection: isolation and bootstrap', flush=True)
    assert not any((p / 'device').exists() for p in Path('/sys/class/net').iterdir())
    for family in ('-4', '-6'):
        assert json.loads(command('ip', '-j', family, 'route', 'show', 'default')) == []
    for unit in ('docker-agent-one.service', 'docker-agent-two.service', 'hostpulse-system.service'):
        eventually('unit ' + unit, lambda: command('systemctl', 'is-active', unit) == 'active')
    assert not Path('/var/lib/tentaflake-workloads/authority.json').exists()
    service('stop', 'tentaflake-workloads.service')
    service('start', 'tentaflake-workloads-initialize.service')
    service('reset-failed', 'tentaflake-workloads.service')
    service('start', 'tentaflake-workloads.service')
    one, two = live('one'), live('two')
    assert one['cgroup'] != two['cgroup'] and one['pid'] != two['pid']
    assert one['container_id'] != two['container_id']
    first('one')
    first('two')
    for name, maximum in [('one', 96 * 1024**2), ('two', 160 * 1024**2)]:
        kernel = counters(name)
        value = available(name)
        validate(name, value)
        row = value['samples'][-1]
        assert row['memory_max_bytes'] == kernel['maximum'] == maximum
        assert abs(row['memory_current_bytes'] - kernel['memory']) < 8 * 1024**2
    assert counters('two')['memory'] > counters('one')['memory'] + 16 * 1024**2
    for user, name in [('reader-one', 'two'), ('reader-two', 'one')]:
        value = read(name, user)
        assert value['status'] == 'unauthorized' and value['resource'] is None and value['samples'] == []
    for user in ('root', 'outsider', 'hostpulse'):
        assert read('one', user) == {'closed': True}, 'exact UID only; no root bypass'
    pid = int(command('systemctl', 'show', 'hostpulse-system.service', '-p', 'MainPID', '--value'))
    status = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines())
    assert set(map(int, status['Uid'].split())) == {pwd.getpwnam('hostpulse').pw_uid}
    assert int(status['CapEff'], 16) == int(status['CapBnd'], 16) == 0
    assert status['NoNewPrivs'].strip() == '1'
    assert status['Groups'].strip() in ('', str(pwd.getpwnam('hostpulse').pw_gid))
    command('docker', '--host=unix:///run/docker.sock', 'ps', user='hostpulse', success=False)
    assert POLICY.stat().st_uid == 0 and POLICY.stat().st_mode & 0o777 == 0o644
    command(sys.executable, '-c', 'from pathlib import Path; Path("' + str(POLICY) + '").open("a")',
            user='hostpulse', success=False)

    print('synthetic projection: bounded CPU/memory/IO kernel comparison', flush=True)
    before = counters('one')
    command('docker', 'exec', 'agent-one', '/bin/python3', __file__, 'load', timeout=20)
    time.sleep(2)
    value = available('one')
    after = counters('one')
    validate('one', value)
    rows = [row for row in value['samples'] if before['time'] <= row['captured_at_ms'] <= after['time']]
    assert len(rows) >= 4
    for field, delta in [
        ('cpu_usage_us_per_second', after['cpu'] - before['cpu']),
        ('io_read_bytes_per_second', sum(v[0] for v in after['io'].values()) - sum(v[0] for v in before['io'].values())),
        ('io_write_bytes_per_second', sum(v[1] for v in after['io'].values()) - sum(v[1] for v in before['io'].values())),
    ]:
        pairs = [(a, b) for a, b in zip(rows, rows[1:]) if b[field] is not None]
        assert pairs, 'kernel IO counters must become observable, not silently zero-filled'
        integrated = sum(b[field] * (b['captured_at_ms'] - a['captured_at_ms']) / 1000 for a, b in pairs)
        # The integration window is strictly inside the independently read kernel
        # window. 15% tolerates wall/boot clock scheduling and ms quantization.
        assert integrated <= delta * 1.15 + 10000, 'scoped rate exceeds kernel delta'
        if field != 'io_read_bytes_per_second':
            assert delta > 0 and integrated > delta * 0.15, 'bounded load absent from scoped rates'
    # Cached reads may legitimately produce zero kernel reads, never fabricated IO.

    print('synthetic projection: reset, stop and restart', flush=True)
    # Deliberate synthetic operator restarts clear the start-rate counter;
    # production retains its unchanged 60-second/3-start limit.
    service('reset-failed', 'hostpulse-system.service')
    service('restart', 'hostpulse-system.service')
    first('one')
    service('stop', 'docker-agent-one.service')
    eventually('stopped manifest', lambda: fact('one')['state'] == 'unavailable')
    unavailable('one')
    validate('two', available('two'))
    service('start', 'docker-agent-one.service')
    assert live('one')['generation'] > one['generation']
    first('one')

    print('synthetic projection: lease and corrupted provenance', flush=True)
    service('stop', 'tentaflake-workloads.service')
    time.sleep(11)
    unavailable('one')
    service('start', 'tentaflake-workloads.service')
    live('one')
    first('one')
    service('stop', 'tentaflake-workloads.service')
    path = FACTS / 'agent-one.json'
    saved = path.read_bytes()
    original = json.loads(saved)
    for field, bad in [('tenant_id', 'tenant-two'), ('cgroup_inode', original['cgroup_inode'] + 1),
                       ('cgroup', '/'), ('generation', 0)]:
        damaged = dict(original, verified_at_ms=time.time_ns() // 1_000_000)
        damaged[field] = bad
        path.write_text(json.dumps(damaged))
        unavailable('one')
    path.write_text('invalid')
    unavailable('one')
    path.write_bytes(saved)
    os.chmod(path, 0o666)
    unavailable('one')
    os.chmod(path, 0o444)
    service('start', 'tentaflake-workloads.service')
    live('one')
    first('one')

    print('synthetic projection: runtime reader revocation and authority corruption', flush=True)
    policy = POLICY.read_text()
    POLICY.write_text(policy.replace('uid = 2100', 'uid = 2102'))
    unavailable('one')  # changed root policy invalidates in-memory grants immediately
    service('reset-failed', 'hostpulse-system.service')
    service('restart', 'hostpulse-system.service')
    assert read('one', 'reader-one')['status'] == 'unauthorized'
    value = eventually('new exact reader', lambda: read('one', 'outsider') if read('one', 'outsider').get('status') == 'ok' else None)
    validate('one', value)
    POLICY.write_text(policy)
    service('reset-failed', 'hostpulse-system.service')
    service('restart', 'hostpulse-system.service')
    first('one')
    service('stop', 'tentaflake-workloads.service')
    authority = Path('/var/lib/tentaflake-workloads/authority.json')
    authority.write_text('invalid')
    service('start', 'tentaflake-workloads.service')
    time.sleep(11)
    unavailable('one')
    command('systemctl', 'start', 'tentaflake-workloads-initialize.service', success=False)
    assert authority.read_text() == 'invalid'
    service('stop', 'hostpulse-system.service', 'tentaflake-workloads.service',
            'docker-agent-one.service', 'docker-agent-two.service')
    print('synthetic projection passed; UI HTTP and native run attribution NOT asserted', flush=True)


if __name__ == '__main__':
    mode = sys.argv[1]
    if mode == 'query':
        print(json.dumps(query(sys.argv[2])))
    else:
        {'provision': provision, 'workload': workload, 'load': load, 'test': test}[mode]()
