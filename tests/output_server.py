#!/usr/bin/env python3
"""Private PipeWire/Pulse server, policy only, two null sinks. No physical devices."""
import json, os, shutil, subprocess, sys, tempfile, time
from pathlib import Path
binary=Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='flacker-output-') as tmp:
    root=Path(tmp); runtime=root/'runtime';runtime.mkdir(mode=0o700);conf=root/'config';conf.mkdir()
    for name in ('pipewire.conf','pipewire-pulse.conf'):shutil.copyfile(Path('/usr/share/pipewire')/name,conf/name)
    env=dict(os.environ,XDG_RUNTIME_DIR=str(runtime),PIPEWIRE_RUNTIME_DIR=str(runtime),PIPEWIRE_REMOTE='pipewire-0',PIPEWIRE_CONFIG_DIR=str(conf),XDG_CONFIG_HOME=str(root/'user-config'),XDG_STATE_HOME=str(root/'state'),PULSE_SERVER=f'unix:{runtime}/pulse/native')
    env.pop('FLACKER_SILENT',None)
    processes=[]
    with (root/'server.log').open('w+') as log:
        try:
            processes.append(subprocess.Popen(['pipewire'],env=env,stdout=log,stderr=log));time.sleep(.4)
            processes.append(subprocess.Popen(['wireplumber','--profile','policy'],env=env,stdout=log,stderr=log));time.sleep(.4)
            processes.append(subprocess.Popen(['pipewire-pulse'],env=env,stdout=log,stderr=log))
            def pactl(*args,check=True):return subprocess.run(['pactl',*args],env=env,check=check,capture_output=True,text=True,timeout=5)
            for _ in range(30):
                time.sleep(.2)
                if pactl('info',check=False).returncode==0:break
            else:raise AssertionError('Private Pulse server did not start')
            for name in ('a','b'):pactl('load-module','module-null-sink',f'sink_name=flacker_test_{name}',f'sink_properties=device.description=Flacker_Test_{name.upper()}')
            time.sleep(.5)
            sinks=json.loads(pactl('-f','json','list','sinks').stdout)
            assert all(s['name'].startswith('flacker_test_') or s['name']=='auto_null' for s in sinks), 'Unexpected hardware sink'
            track=root/'signal.flac';subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-f','lavfi','-i','sine=frequency=440:sample_rate=96000:duration=20','-c:a','flac','-sample_fmt','s32',str(track)],check=True)
            client=subprocess.Popen([str(binary),'--audio-output-test',str(track)],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                import selectors
                selector=selectors.DefaultSelector();selector.register(client.stdout,selectors.EVENT_READ)
                assert selector.select(20), 'Output test did not reach disconnect stage'
                line=client.stdout.readline();assert line.strip()=='READY_DISCONNECT',line+client.stderr.read() if client.poll() is not None else line
                processes[-1].terminate();processes[-1].wait(timeout=5)
                stdout,stderr=client.communicate(timeout=10);assert client.returncode==0,stdout+stderr
                print(stdout.strip())
            finally:
                if client.poll() is None:client.kill();client.wait()
        except BaseException:
            log.seek(0);print(log.read(),file=sys.stderr);raise
        finally:
            for process in reversed(processes):
                if process.poll() is None:process.terminate()
            for process in reversed(processes):
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait()
