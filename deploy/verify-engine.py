#!/usr/bin/env python3
"""Docker regression test for HTTP-200 HTML/truncated AES HLS segments.

Run: python3 deploy/verify-engine.py pagecatch:0.1.0-amd64
Requires Docker and OpenSSL on the host. Uses only synthetic local video.
"""
import argparse
import collections
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading


def run(args, **kwargs):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image')
    parser.add_argument('--expect-old-failure', action='store_true')
    args = parser.parse_args()
    # Bind-mounted fixtures must remain writable/removable by the Linux CI user.
    user = ['--user', f'{os.getuid()}:{os.getgid()}'] if hasattr(os, 'getuid') else []
    with tempfile.TemporaryDirectory(prefix='pagecatch-engine-test-') as tmp:
        folder = Path(tmp)
        run(['docker', 'run', '--rm', *user, '--entrypoint', 'ffmpeg',
             '-v', f'{folder}:/fixture', args.image,
             '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10',
             '-f', 'lavfi', '-i', 'sine=frequency=500', '-t', '8',
             '-c:v', 'libx264', '-threads', '1', '-g', '20', '-sc_threshold', '0',
             '-c:a', 'aac', '-hls_time', '2', '-hls_playlist_type', 'vod',
             '-hls_segment_filename', '/fixture/plain%02d.ts', '/fixture/plain.m3u8'])
        key = bytes.fromhex('00112233445566778899aabbccddeeff')
        plain = sorted(folder.glob('plain*.ts'))
        encrypted = []
        for index, source in enumerate(plain):
            target = folder / f'enc{index}.ts'
            run(['openssl', 'enc', '-aes-128-cbc', '-K', key.hex(),
                 '-iv', index.to_bytes(16, 'big').hex(), '-in', str(source), '-out', str(target)])
            encrypted.append(target.read_bytes())
        assert len(plain) == 4
        playlist = (folder / 'plain.m3u8').read_text()
        playlist = playlist.replace('#EXT-X-MEDIA-SEQUENCE:0',
                                    '#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI="key"')
        for index, source in enumerate(plain):
            playlist = playlist.replace(source.name, f'segment{index}.ts')
        counts = collections.Counter()
        lock = threading.Lock()
        html = b'<!DOCTYPE html><html><title>Fake media response</title></html>'
        cases = ['clean', 'html_once', 'padded_html_once', 'truncated_once',
                 'bad_padding_once', 'empty_once', 'html_first_once',
                 'html_unencrypted_once', 'html_always']

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                parts = self.path.strip('/').split('/')
                if len(parts) != 2 or parts[0] not in cases:
                    self.send_error(404)
                    return
                case, name = parts
                if self.headers.get('X-Fixture') != 'pagecatch-test':
                    self.send_error(403)
                    return
                with lock:
                    counts[(case, name)] += 1
                    count = counts[(case, name)]
                content_type = 'application/octet-stream'
                if name == 'playlist.m3u8':
                    data = playlist.encode()
                    if case == 'html_unencrypted_once':
                        data = b'\n'.join(line for line in data.split(b'\n')
                                          if not line.startswith(b'#EXT-X-KEY:'))
                    content_type = 'application/vnd.apple.mpegurl'
                elif name == 'key':
                    data = key
                elif name.startswith('segment') and name.endswith('.ts'):
                    index = int(name[7:-3])
                    if not 0 <= index < len(encrypted):
                        self.send_error(404)
                        return
                    data = encrypted[index]
                    if case == 'html_unencrypted_once':
                        data = plain[index].read_bytes()
                    bad_index = 0 if case == 'html_first_once' else 1
                    if index == bad_index and (count == 1 or case == 'html_always'):
                        if case in ['html_once', 'html_always', 'html_first_once', 'html_unencrypted_once']:
                            data = html
                            content_type = 'text/html'
                        elif case == 'padded_html_once':
                            data = html.ljust(128, b' ')
                            content_type = 'text/html'
                        elif case == 'truncated_once':
                            data = data[:-1]
                        elif case == 'bad_padding_once':
                            # Change the final plaintext padding byte via the preceding CBC block.
                            changed = bytearray(data)
                            changed[-17] ^= 0x80
                            data = bytes(changed)
                        elif case == 'empty_once':
                            data = b''
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header('Content-Type', content_type)
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        # Docker's host gateway must reach this synthetic fixture server on Linux.
        server = http.server.ThreadingHTTPServer(('0.0.0.0', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for case in cases[:2] if args.expect_old_failure else cases:
                output = folder / case
                output.mkdir()
                url = f'http://host.docker.internal:{server.server_port}/{case}/playlist.m3u8'
                command = ['docker', 'run', '--rm', *user, '-t', '-e', 'TERM=xterm',
                           '--add-host', 'host.docker.internal:host-gateway',
                           '--entrypoint', 'N_m3u8DL-RE', '-v', f'{output}:/check',
                           args.image, url, '--save-dir', '/check', '--tmp-dir', '/check',
                           '--save-name', 'video', '--auto-select', '--no-log',
                           '--write-meta-json', 'false', '--disable-update-check',
                           '--del-after-done', 'false', '--thread-count', '2',
                           '--download-retry-count', '2', '--http-request-timeout', '5',
                           '-H', 'X-Fixture: pagecatch-test', '-M', 'format=mp4']
                result = subprocess.run(command, stdout=subprocess.PIPE,
                                        stderr=subprocess.STDOUT, timeout=45)
                log = result.stdout.decode(errors='replace')
                if args.expect_old_failure and case == 'html_once':
                    assert result.returncode != 0 and 'not a complete block' in log, log[-3000:]
                    print('OLD ENGINE FAILURE REPRODUCED: HTTP-200 HTML aborts AES HLS download.', flush=True)
                    return
                requests = {name: count for (which, name), count in counts.items() if which == case}
                if case == 'html_always':
                    assert result.returncode != 0, f'exit={result.returncode}; {log[-1000:]}'
                    assert not (output / 'video.mp4').exists(), 'Incomplete video was published'
                    assert requests['segment1.ts'] == 3, requests
                    assert 'HTML' in log and 'Unhandled exception' not in log, log[-1000:]
                    assert 'Failed to write:' not in log, 'Warning markup failed to render'
                else:
                    assert result.returncode == 0, log[-3000:]
                    assert (output / 'video.mp4').stat().st_size > 1024
                    for index, source in enumerate(plain):
                        match = list((output / 'video').rglob(f'{index}.ts'))
                        assert len(match) == 1, list(output.rglob('*'))
                        assert hashlib.sha256(match[0].read_bytes()).digest() == hashlib.sha256(source.read_bytes()).digest()
                        bad_index = 0 if case == 'html_first_once' else 1
                        expected_count = 2 if index == bad_index and case != 'clean' else 1
                        assert requests[f'segment{index}.ts'] == expected_count, requests
                    run(['docker', 'run', '--rm', *user, '--entrypoint', 'ffmpeg',
                         '-v', f'{output}:/check:ro', args.image, '-v', 'error',
                         '-threads', '1', '-err_detect', 'explode', '-xerror',
                         '-i', '/check/video.mp4', '-f', 'null', '-'])
                print(json.dumps({'case': case, 'passed': True,
                                  'exit': result.returncode, 'requests': requests}), flush=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    main()
