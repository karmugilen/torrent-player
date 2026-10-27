#!/usr/bin/env python3
"""Print side-by-side latest Go/Rust public-swarm samples as Markdown."""

import argparse
import json


def number(value, scale=1):
    return 'unavailable' if value is None else f'{value * scale:.3f}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('results', nargs='+')
    args = parser.parse_args()
    latest = {}
    for path in args.results:
        for result in json.load(open(path, encoding='utf-8')):
            if result.get('requested_seconds', 60) != 60 or result.get('peers_only'):
                continue
            if 0 < result.get('download_window_seconds', 60) < 55:
                continue
            key = (result['sample'], result['mode'], result.get('storage', 'stream'), result['label'])
            if key not in latest or result['started_utc'] > latest[key]['started_utc']:
                latest[key] = result
    print('| Sample / mode / storage | Engine | Metadata s | First byte s | 60 s MiB/s | Seek s (5/25/50/75/95%) | Peak RSS MiB | CPU s | Host flow |')
    print('| --- | --- | ---: | ---: | ---: | --- | ---: | ---: | --- |')
    for sample, mode, storage in sorted({key[:3] for key in latest}):
        for label in ('go', 'rust'):
            result = latest.get((sample, mode, storage, label), {})
            seeks = result.get('seeks', [])
            seek_text = ', '.join(number(seek.get('first_byte_seconds')) for seek in seeks) or 'unavailable'
            print(f"| {sample} / {mode} / {storage} | {label} | {number(result.get('metadata_seconds'))} | "
                  f"{number(result.get('first_stream', {}).get('first_byte_seconds'))} | "
                  f"{number(result.get('download_mib_per_sec'))} | {seek_text} | "
                  f"{number(result.get('peak_rss_mib'))} | {number(result.get('cpu_seconds'))} | "
                  f"{'PASS' if result.get('passed_host_flow') else 'FAIL' if result else 'unavailable'} |")


if __name__ == '__main__':
    main()
