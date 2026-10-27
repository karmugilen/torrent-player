#!/usr/bin/env python3
"""Install newest APK, exercise exported magnet VIEW flow, retain bounded logcat.

Never uninstall, clear app data, clear logcat, or touch an unauthorized device.
An absent device is explicitly unverified, not success.
"""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import time

from smoke import sample


def adb(*args):
    result = subprocess.run(['adb', *args], text=True, capture_output=True, timeout=120)
    return dict(command=['adb', *args], returncode=result.returncode,
                stdout=result.stdout, stderr=result.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, default=Path('/home/kar/funProjects/torrent-player/artifacts'))
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=45)
    args = parser.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    result = dict(started_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), calls=[])
    devices = adb('devices')
    result['calls'].append(devices)
    serials = [line.split()[0] for line in devices['stdout'].splitlines()[1:] if line.endswith('\tdevice')]
    result['devices'] = serials
    if len(serials) != 1:
        result['status'] = 'unverified: need exactly one authorized attached device'
        args.out.write_text(json.dumps(result, indent=2) + '\n')
        print(result['status'])
        return 2
    apks = sorted(args.artifacts.glob('*.apk'), key=lambda path: path.stat().st_mtime, reverse=True)
    if not apks:
        raise SystemExit('no APK found')
    result['apk'] = str(apks[0])
    install = adb('-s', serials[0], 'install', '-r', str(apks[0]))
    result['calls'].append(install)
    if install['returncode']:
        result['status'] = 'failed: APK installation (data preserved)'
    else:
        # Start from current time; do not clear the shared logcat buffer.
        timestamp = adb('-s', serials[0], 'shell', 'date "+%m-%d %H:%M:%S.000"')['stdout'].strip()
        magnet, _ = sample('sintel')
        result['calls'].append(adb('-s', serials[0], 'shell',
                                   'am start -W -a android.intent.action.VIEW -d ' + shlex.quote(magnet) + ' -p webtor.app'))
        time.sleep(min(max(args.seconds, 1), 60))
        result['calls'].append(adb('-s', serials[0], 'shell', 'dumpsys activity activities'))
        logs = adb('-s', serials[0], 'logcat', '-d', '-t', '4000', '-T', timestamp, '-v', 'threadtime')
        needles = ('webtor', 'engine', 'fatal exception', 'fatal signal', 'androidruntime', 'metadata', 'torrent')
        logs['stdout'] = '\n'.join(line for line in logs['stdout'].splitlines()
                                    if any(word in line.lower() for word in needles))
        result['calls'].append(logs)
        result['status'] = 'captured: inspect VIEW result and logs; native preview/Play unverified'
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(result['status'])
    return int(result['status'].startswith('failed'))


if __name__ == '__main__':
    raise SystemExit(main())
