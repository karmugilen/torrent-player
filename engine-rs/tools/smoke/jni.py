#!/usr/bin/env python3
"""Exercise real JNI preview/playback APIs over public swarms with CheckJNI."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import resource
import shutil
import subprocess
import time
import urllib.request

from smoke import ROOT, bdecode, sample


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--label', choices=['go', 'rust'], required=True)
    parser.add_argument('--samples', nargs='+', default=['sintel', 'big-buck-bunny'])
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    ROOT.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(ROOT).free < 25 * 1024**3:
        raise SystemExit('less than 25 GiB disk free')
    classes = ROOT / 'jni-classes'
    classes.mkdir(exist_ok=True)
    source = Path(__file__).parent / 'jni/webtor/app/EngineHost.java'
    subprocess.run(['javac', '-Xlint:all', '-Werror', '-d', str(classes), str(source)], check=True)
    results = []
    args.out.parent.mkdir(parents=True, exist_ok=True)
    try:
        for name in args.samples:
            magnet, url = sample(name)
            with urllib.request.urlopen(url, timeout=30) as response:
                metadata = bdecode(response.read(2 * 1024 * 1024))
            files = metadata[b'info'].get(b'files', [metadata[b'info']])
            choices = [(index, file) for index, file in enumerate(files)
                       if b'/'.join(file.get(b'path', [metadata[b'info'][b'name']])).lower().endswith((b'.mp4', b'.mkv', b'.webm'))]
            index, video = max(choices, key=lambda pair: pair[1][b'length'])
            work = ROOT / f'jni-{args.label}-{name}'
            work.mkdir(exist_ok=False)
            command = ['java', '-Xcheck:jni', '-Xmx128m', '-cp', str(classes), 'webtor.app.EngineHost',
                       str(args.library.resolve()), str(work), base64.b64encode(magnet.encode()).decode(),
                       str(len(files)), str(index), str(video[b'length'])]
            print(f'RUN JNI {args.label} {name}', flush=True)
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            start = time.monotonic()
            try:
                process = subprocess.run(command, text=True, capture_output=True, timeout=240)
                events = []
                for line in process.stdout.splitlines():
                    try:
                        events.append(json.loads(line))
                    except json.JSONDecodeError:
                        events.append(dict(kind='raw', text=line))
                result = dict(label=args.label, sample=name, returncode=process.returncode,
                              events=events, stderr=process.stderr[-24000:],
                              passed_native_flow=process.returncode == 0,
                              android_decoder='unverified: desktop JVM has no MediaMetadataRetriever')
            except subprocess.TimeoutExpired as error:
                result = dict(label=args.label, sample=name, passed_native_flow=False,
                              error='240-second JNI process timeout', stdout=str(error.stdout)[-24000:],
                              stderr=str(error.stderr)[-24000:])
            finally:
                shutil.rmtree(work)
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            result.update(seconds=time.monotonic() - start,
                          cpu_seconds=(after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime),
                          peak_rss_mib=after.ru_maxrss / 1024,
                          rss_scope='RUSAGE_CHILDREN maximum for this wrapper (includes javac)',
                          library_sha256=hashlib.sha256(args.library.read_bytes()).hexdigest(),
                          payload_deleted=not work.exists())
            results.append(result)
            args.out.write_text(json.dumps(results, indent=2) + '\n')
            print(json.dumps({key: result[key] for key in ('label', 'sample', 'passed_native_flow', 'seconds')}), flush=True)
    finally:
        shutil.rmtree(classes)
    return int(any(not result['passed_native_flow'] for result in results))


if __name__ == '__main__':
    raise SystemExit(main())
