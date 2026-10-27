#!/usr/bin/env python3
"""Exercise Android's control/HTTP flow on public legal swarms (stdlib only).

No local seeder, metadata injection, tracker replacement, or preflight metadata
wait in the immediate-configure regression. Results retain every control reply.
Native preview and proxy-FD playback require Android; see phone.py and SMOKE.md.
"""

import argparse
import base64
import hashlib
import html.parser
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path('/home/kar/.cache/wt-tmp/codex-smoke')
MIB = 1024 * 1024
PUBLIC = 'https://webtorrent.io/free-torrents'


class Links(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.magnets = []

    def handle_starttag(self, tag, attrs):
        link = dict(attrs).get('href', '')
        if tag == 'a' and link.startswith('magnet:'):
            self.magnets.append(link)


def sample(name):
    parser = Links()
    with urllib.request.urlopen(PUBLIC, timeout=30) as response:
        parser.feed(response.read(256 * 1024).decode())
    for magnet in parser.magnets:
        query = urllib.parse.parse_qs(urllib.parse.urlparse(magnet).query)
        if query.get('dn', [''])[0].lower().replace(' ', '-') == name:
            return magnet, query['xs'][0]
    raise ValueError(f'{name} missing from {PUBLIC}')


def bdecode(data):
    """Read official fixture structure, never inject metadata into a magnet."""
    position = 0

    def read():
        nonlocal position
        token = data[position:position + 1]
        position += 1
        if token == b'i':
            end = data.index(b'e', position)
            value = int(data[position:end])
            position = end + 1
            return value
        if token in (b'l', b'd'):
            values = []
            while data[position:position + 1] != b'e':
                values.append(read())
            position += 1
            return dict(zip(values[::2], values[1::2])) if token == b'd' else values
        position -= 1
        end = data.index(b':', position)
        size = int(data[position:end])
        position = end + 1 + size
        return data[end + 1:position]

    return read()


class ApiError(RuntimeError):
    pass


class Host:
    def __init__(self, binary, work, record, descriptors=()):
        self.binary, self.work, self.record = binary, work, record
        self.process = None
        self.stop = threading.Event()
        self.peak_rss = 0
        self.cpu = 0
        self.safety = None
        self.id = None
        self.peer_bytes = 0
        self.peer_previous = {}
        self.started = time.monotonic()
        self.descriptors = descriptors

    def start(self):
        self.work.mkdir(parents=True, exist_ok=True)
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            self.port = reservation.getsockname()[1]
        self.log = (self.work / 'host.log').open('wb')
        self.process = subprocess.Popen(
            [self.binary, '-port', str(self.port), '-path', str(self.work / 'data'), '-max-peers', '55'],
            stdout=self.log, stderr=subprocess.STDOUT,
            pass_fds=self.descriptors,
        )
        self.stop.clear()
        self.monitor = threading.Thread(target=self.watch, daemon=True)
        self.monitor.start()
        for _ in range(200):
            if self.process.poll() is not None:
                raise ApiError(f'host exited {self.process.returncode}')
            try:
                self.call('GET', '/stats', timeout=1, trace=False)
                return
            except (OSError, ApiError):
                time.sleep(.1)
        raise ApiError('host failed to start within 20 seconds')

    def watch(self):
        # A conservative 128 MiB stop leaves ample headroom below the 200 MiB
        # per-download ceiling for queued blocks. Limit also applies on failure.
        while not self.stop.wait(.1):
            try:
                stat = Path(f'/proc/{self.process.pid}/stat').read_text().rsplit(')', 1)[1].split()
                self.cpu = (int(stat[11]) + int(stat[12])) / os.sysconf('SC_CLK_TCK')
                self.peak_rss = max(self.peak_rss, int(stat[21]) * os.sysconf('SC_PAGE_SIZE'))
                if time.monotonic() - self.started > 360:
                    self.safety = '360 second process deadline'
                if self.id:
                    peers = self.call('GET', f'/connections/{self.id}', timeout=.5, trace=False)
                    for peer in peers.get('peers') or []:
                        key = peer['address']
                        count = peer.get('downloaded', 0)
                        previous = self.peer_previous.get(key, 0)
                        self.peer_bytes += max(0, count - previous) if count >= previous else count
                        self.peer_previous[key] = count
                allocated = sum(p.stat().st_blocks * 512 for p in self.work.rglob('*') if p.is_file())
                if max(self.peer_bytes, allocated) >= 128 * MIB:
                    self.safety = '128 MiB safety stop (200 MiB ceiling)'
                if self.safety:
                    self.process.kill()
                    return
            except (OSError, ValueError, KeyError, ApiError):
                continue

    def call(self, method, path, body=None, timeout=65, trace=True):
        request = urllib.request.Request(
            f'http://127.0.0.1:{self.port}{path}', method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type': 'application/json'},
        )
        start = time.monotonic()
        try:
            response = urllib.request.urlopen(request, timeout=timeout)
        except urllib.error.HTTPError as error:
            response = error
        except (OSError, TimeoutError) as error:
            if trace:
                self.record['calls'].append({'method': method, 'path': path, 'request': body,
                                             'seconds': time.monotonic() - start, 'error': str(error)})
            raise
        with response:
            payload = response.read(4 * MIB)
            value = json.loads(payload)
            if trace:
                self.record['calls'].append({'method': method, 'path': path, 'request': body,
                                             'status': response.status, 'seconds': time.monotonic() - start,
                                             'response': value})
            if response.status >= 400:
                raise ApiError(f'{method} {path}: {response.status} {value}')
            return value

    def close(self):
        if self.process:
            self.stop.set()
            self.monitor.join(timeout=2)
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    self.process.kill()
            self.process.wait()
            self.record.setdefault('host_exit_codes', []).append(self.process.returncode)
            self.log.close()
            self.record['host_log_tail'] = (self.work / 'host.log').read_text(errors='replace')[-16000:]
            self.record['cpu_seconds'] = self.record.get('cpu_seconds', 0) + self.cpu
            self.record['peak_rss_mib'] = max(self.record.get('peak_rss_mib', 0), self.peak_rss / MIB)
            self.record['observed_peer_download_mib'] = self.record.get('observed_peer_download_mib', 0) + self.peer_bytes / MIB
            if self.safety:
                self.record['safety_stop'] = self.safety


def settings(host, profile):
    # Order is LibrarySession initialization; values are controlled smoke settings.
    values = [dict(maxPeers=55), dict(downloadRateBytesSec=512 * 1024), dict(dialRatePerSec=30),
              dict(peerExploration=False), dict(ghostMode=False), dict(ghostProMode=profile == 'app'),
              dict(claudeSeekMode=profile == 'app'), dict(hedgeRequests=profile == 'app'), dict(slowPeerYield=profile == 'app'),
              dict(dropChokers=profile == 'app'), dict(idleDropMinutes=15 if profile == 'app' else 0), dict(ramCacheBytes=32 * MIB),
              dict(seedingEnabled=profile == 'app', seedingAllowed=profile == 'app', seedRatioMilli=1000,
                   seedTimeMinutes=60, seedUploadKiB=256), dict(payloadTransfersAllowed=True)]
    for value in values:
        host.call('POST', '/settings', value)
    host.call('POST', '/settings', dict(maxPeers=64))
    current = host.call('GET', '/settings')
    expect(current['downloadRateBytesSec'] == 512 * 1024, 'maxPeers update changed download limit')
    host.call('POST', '/settings', dict(maxPeers=55))
    host.call('POST', '/settings', dict(maxPeers=0))
    expect(host.call('GET', '/settings')['maxPeers'] == 55, 'maxPeers zero must preserve the previous limit')
    host.call('GET', '/cache')
    host.call('POST', '/bypass', dict(enabled=False))
    host.call('POST', '/network', dict(generation=1, connected=True))


def ready(host, torrent_id, timeout=90):
    start = time.monotonic()
    while time.monotonic() - start < timeout:
        status = host.call('GET', f'/torrent/{torrent_id}')
        if status.get('error'):
            raise ApiError(str(status['error']))
        if status.get('ready') and status.get('files'):
            return status
        # JNI uses awaitChange; host has no event endpoint. Poll at 250 ms.
        time.sleep(.25)
    raise ApiError(f'metadata not ready after {timeout}s')


def expect(value, message):
    if not value:
        raise ApiError(message)


def stream(url, offset, length=65536):
    start = time.monotonic()
    request = urllib.request.Request(url, headers={'Range': f'bytes={offset}-{offset + length - 1}'})
    with urllib.request.urlopen(request, timeout=20) as response:
        expect(response.status == 206, f'range returned {response.status}')
        expect(response.headers.get('Content-Range', '').startswith(f'bytes {offset}-'), 'incorrect Content-Range')
        data = response.read(1)
        first = time.monotonic() - start
        expect(len(data) == 1, 'stream returned EOF before first byte')
        data += response.read(length - 1)
        expect(len(data) == length, f'short stream read: {len(data)} != {length}')
        return dict(offset=offset, first_byte_seconds=first, read_seconds=time.monotonic() - start,
                    bytes=len(data), sha256=hashlib.sha256(data).hexdigest())


def run_case(args, name, mode):
    record = dict(label=args.label, sample=name, mode=mode, storage=args.storage, calls=[], errors=[],
                  profile=args.profile,
                  peers_only=args.peers_only,
                  requested_seconds=args.seconds,
                  started_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                  binary=str(Path(args.engine).resolve()),
                  binary_sha256=hashlib.sha256(Path(args.engine).read_bytes()).hexdigest(),
                  driver_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  native_preview='unverified: JNI only, not an HTTP route')
    work = ROOT / f'{args.label}-{name}-{mode}-{os.getpid()}'
    host = Host(args.engine, work, record)
    descriptors = []
    try:
        magnet, torrent_url = sample(name)
        if args.peers_only:
            parts = urllib.parse.urlsplit(magnet)
            query = [(key, value) for key, value in urllib.parse.parse_qsl(parts.query)
                     if key not in ('xs', 'as', 'ws')]
            magnet = urllib.parse.urlunsplit(parts._replace(query=urllib.parse.urlencode(query)))
        record['source'] = magnet
        torrent_data = None
        if mode in ('torrent', 'immediate') or args.storage == 'download':
            with urllib.request.urlopen(torrent_url, timeout=30) as response:
                torrent_data = response.read(2 * MIB)
        if args.storage == 'download':
            count = len(bdecode(torrent_data)[b'info'].get(b'files', [None]))
            (work / 'destinations').mkdir(parents=True)
            descriptors = [os.open(work / 'destinations' / str(i), os.O_RDWR | os.O_CREAT, 0o600) for i in range(count)]
            host.descriptors = descriptors
        host.start()
        settings(host, args.profile)
        body = dict(torrentId=magnet, prepare=True)
        if mode == 'torrent':
            body['torrentData'] = base64.b64encode(torrent_data).decode()
            body['torrentId'] = str(work / (name + '.torrent'))
        start = time.monotonic()
        torrent_id = host.call('POST', '/add', body)['id']
        host.id = torrent_id
        if mode == 'immediate':
            # Shape comes from the public fixture, without giving the host metadata.
            count = len(bdecode(torrent_data)[b'info'].get(b'files', [None]))
            # This request must block, not return "metadata not ready" immediately.
            host.call('POST', '/configure', dict(id=torrent_id, descriptors=descriptors or [None] * count, selected=[],
                                                 storageMode=args.storage, streamCacheBytes=32 * MIB))
        status = ready(host, torrent_id)
        record['metadata_seconds'] = time.monotonic() - start
        files = status['files']
        video = max((f for f in files if f.get('name', '').lower().endswith(('.mp4', '.mkv', '.webm', '.avi'))),
                    key=lambda f: f['length'], default=max(files, key=lambda f: f['length']))
        index = video['index']
        selected = [index]
        try:
            host.call('POST', '/select', dict(id=torrent_id, selected=selected))
        except ApiError:
            # waitReady/syncPrepareSelection deliberately runCatching this call:
            # Go rejects selecting destinations before configure (409).
            host.record['calls'][-1]['tolerated_by_kotlin'] = True
        metadata = host.call('GET', f'/metadata/{torrent_id}')['torrentData']
        config = dict(descriptors=descriptors or [None] * len(files), selected=selected,
                      storageMode=args.storage, streamCacheBytes=32 * MIB)
        if mode != 'immediate':
            host.call('POST', '/configure', dict(id=torrent_id, **config))
        host.call('POST', '/select', dict(id=torrent_id, selected=[]))
        expect(host.call('GET', f'/torrent/{torrent_id}')['selected'] in ([], None), 'empty selection was not applied')
        selection = dict(id=torrent_id, selected=selected)
        if descriptors:
            selection['descriptors'] = descriptors
        host.call('POST', '/select', selection)
        host.call('POST', '/focus', dict(id=torrent_id, fileIndex=index))
        status = host.call('GET', f'/torrent/{torrent_id}')
        expect(status['configured'], 'configure did not set configured')
        play_options = dict(fileIndex=index, offset=0)
        if args.profile == 'app':
            play_options.update(boost=True, adaptiveBuffer=True)
            if args.storage == 'download':
                play_options['fileOnly'] = True
        play = host.call('POST', '/play', dict(id=torrent_id, **play_options))
        record['stream_url'] = play['streamUrl']
        try:
            record['first_stream'] = stream(play['streamUrl'], 0)
        except (ApiError, OSError) as error:
            record['errors'].append(f'first stream: {error}')
        record['seeks'] = []
        for fraction in (.05, .25, .5, .75, .95):
            offset = int((video['length'] - 65536) * fraction)
            try:
                record['seeks'].append(stream(play['streamUrl'], offset))
            except (ApiError, OSError) as error:
                record['seeks'].append(dict(offset=offset, error=str(error)))
                record['errors'].append(f'seek {fraction}: {error}')
        # Trackers/DHT remain real; no synthetic peers or metadata fallback.
        before = host.peer_bytes
        # Keep a reader open for the whole window, as an actual player does.
        # Without this, Direct Play intentionally becomes idle after startup.
        reader_stop = threading.Event()
        transfer = dict(bytes=0)

        def consume():
            try:
                request = urllib.request.Request(play['streamUrl'], headers={'Range': f"bytes=0-{video['length'] - 1}"})
                with urllib.request.urlopen(request, timeout=20) as response:
                    while not reader_stop.is_set():
                        chunk = response.read(65536)
                        if not chunk:
                            break
                        transfer['bytes'] += len(chunk)
            except OSError as error:
                transfer['error'] = str(error)

        reader = threading.Thread(target=consume, daemon=True)
        reader.start()
        start = time.monotonic()
        record['download_samples'] = []
        while time.monotonic() - start < args.seconds:
            status = host.call('GET', f'/torrent/{torrent_id}')
            record['download_samples'].append(dict(elapsed=time.monotonic() - start, status=status))
            time.sleep(min(1, max(0, args.seconds - (time.monotonic() - start))))
        elapsed = time.monotonic() - start
        reader_stop.set()
        reader.join(timeout=21)
        record['download_window_seconds'] = elapsed
        record['download_mib_per_sec'] = sum(s['status'].get('downloadSpeed', 0) for s in record['download_samples']) / max(1, len(record['download_samples'])) / MIB
        record['peer_mib_per_sec'] = (host.peer_bytes - before) / MIB / elapsed
        record['stream_mib_per_sec'] = transfer['bytes'] / MIB / elapsed
        record['download_reader'] = transfer
        if transfer['bytes'] == 0:
            record['errors'].append('60-second active stream produced no payload')
        record['download_window_peer_bytes'] = host.peer_bytes - before
        for route, paused in [('/pause', True), ('/resume', False)]:
            host.call('POST', route, dict(id=torrent_id))
            expect(host.call('GET', f'/torrent/{torrent_id}')['paused'] == paused, f'{route} state mismatch')
        host.call('GET', f'/preview-state/{torrent_id}?fileIndex={index}')
        host.call('GET', f'/pieces/{torrent_id}?maxBuckets=120')
        host.call('GET', f'/connections/{torrent_id}')
        host.call('POST', '/file-only', dict(id=torrent_id, enabled=False))
        host.call('POST', '/focus', dict(id=torrent_id, fileIndex=None))
        # Android owns the library and re-adds metadata; the host does not restore it.
        host.call('POST', '/pause', dict(id=torrent_id))
        host.call('POST', '/remove', dict(id=torrent_id, destroyStore=False))
        host.close()
        host = Host(args.engine, work, record, descriptors)
        host.start()
        settings(host, args.profile)
        restored_id = host.call('POST', '/add', dict(torrentId=magnet, prepare=True, torrentData=metadata))['id']
        host.id = restored_id
        ready(host, restored_id, timeout=30)
        host.call('POST', '/configure', dict(id=restored_id, **config))
        host.call('POST', '/resume', dict(id=restored_id))
        restored = host.call('GET', f'/torrent/{restored_id}')
        expect(restored['configured'] and restored['selected'] == selected, 'restore lost configuration/selection')
        restored_play = host.call('POST', '/play', dict(id=restored_id, **play_options))
        record['restored_first_stream'] = stream(restored_play['streamUrl'], 0)
        if 'first_stream' in record:
            expect(record['first_stream']['sha256'] == record['restored_first_stream']['sha256'], 'restore changed stream bytes')
        host.call('POST', '/remove', dict(id=restored_id, destroyStore=True))
        host.call('POST', '/forget-peers', dict(infoHash=restored['infoHash']))
        try:
            host.call('GET', f'/torrent/{restored_id}')
            raise ApiError('removed torrent still exists')
        except ApiError as error:
            expect(': 404 ' in str(error), str(error))
    except (ApiError, OSError, ValueError, KeyError) as error:
        record['errors'].append(str(error))
    finally:
        host.close()
        for descriptor in descriptors:
            os.close(descriptor)
        if work.exists():
            shutil.rmtree(work)
        record['payload_deleted'] = not work.exists()
        record['passed_host_flow'] = not record['errors']
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', required=True)
    parser.add_argument('--label', choices=['go', 'rust'], required=True)
    parser.add_argument('--samples', nargs='+', default=['sintel', 'big-buck-bunny'])
    parser.add_argument('--modes', nargs='+', choices=['magnet', 'immediate', 'torrent'], default=['magnet', 'immediate', 'torrent'])
    parser.add_argument('--seconds', type=int, default=60)
    parser.add_argument('--storage', choices=['stream', 'download'], default='stream')
    parser.add_argument('--profile', choices=['app', 'neutral'], default='app',
                        help='app uses UiState playback/seek defaults; neutral disables experiments')
    parser.add_argument('--peers-only', action='store_true',
                        help='diagnose tracker/DHT/peer delivery by removing xs/as/ws magnet fallbacks')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    expect(0 < args.seconds <= 60, '--seconds must be 1..60')
    ROOT.mkdir(parents=True, exist_ok=True)
    expect(shutil.disk_usage(ROOT).free >= 25 * 1024**3, 'less than 25 GiB disk free')
    results = []
    args.out.parent.mkdir(parents=True, exist_ok=True)
    for name in args.samples:
        for mode in args.modes:
            print(f'RUN {args.label} {name} {mode}', flush=True)
            result = run_case(args, name, mode)
            results.append(result)
            args.out.write_text(json.dumps(results, indent=2) + '\n')
            print(json.dumps({k: result[k] for k in ('label', 'sample', 'mode', 'passed_host_flow', 'errors', 'peak_rss_mib', 'cpu_seconds') if k in result}), flush=True)
    return int(any(not result['passed_host_flow'] for result in results))


if __name__ == '__main__':
    raise SystemExit(main())
