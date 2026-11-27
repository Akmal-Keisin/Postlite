#!/usr/bin/env python3
"""Drive the populated GPUI workbench on a Linux X11 desktop at 1× scale."""

import argparse
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--screenshots", type=Path, help="directory for optional workbench captures")
parser.add_argument("--sessions", action="store_true", help="check OAuth and the cookie drawer")
parser.add_argument("--preview", action="store_true", help="check the native HTML preview")
args = parser.parse_args()
if args.screenshots:
    args.screenshots.mkdir(parents=True, exist_ok=True)
profile = tempfile.TemporaryDirectory(prefix="postlite-gpui-workbench-")
directory = Path(profile.name)

def capture(window, name):
    if args.screenshots:
        subprocess.run(["import", "-window", window, str(args.screenshots / name)], check=True)

sent=[]
tokens=[]
class Echo(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        data=self.rfile.read(int(self.headers["Content-Length"]))
        preview=self.path.partition("?")[0] == "/preview"
        if preview:
            body=b'<h1>Preview fixture</h1><script>hidden script</script>'
        elif self.path == "/token":
            tokens.append(data.decode())
            body=b'{"access_token":"fixture","expires_in":3600}'
        else:
            sent.append((self.path,self.headers.get("X-Scope"),self.headers.get("Authorization"),
                         data.decode(),self.headers.get("Cookie")))
            body=b'{"gpui": "native saved request"}'
        self.send_response(200)
        if args.sessions and self.path != "/token":
            self.send_header("Set-Cookie", "fixture=remembered; Path=/; HttpOnly")
        self.send_header("Content-Type","text/html" if preview else "application/json")
        self.send_header("Content-Length",str(len(body)))
        self.end_headers()
        self.wfile.write(body)

def xdo(*args):
    return subprocess.check_output(["xdotool", *map(str,args)],stderr=subprocess.DEVNULL,text=True)

def clipboard():
    return subprocess.run(["xclip","-selection","clipboard","-out"],
        capture_output=True,text=True,timeout=3).stdout

with http.server.ThreadingHTTPServer(("127.0.0.1",0),Echo) as server:
    threading.Thread(target=server.serve_forever,daemon=True).start()
    request={"kind":"request","id":"request","name":"Station details","url":"{{base_url}}/:id","method":"POST",
        "pathVars":[{"key":"id","value":"{{station}}"}],
        "params":[{"key":"page","value":"{{paging}}","enabled":True}],
        "headers":[{"key":"X-Scope","value":"{{scope}}","enabled":True}],
        "body":{"mode":"raw","contentType":"application/json","raw":"{\"station\": \"{{station}}\"}"}}
    environment={"id":"local","name":"Local","variables":[
        {"key":"base_url","value":f"http://127.0.0.1:{server.server_port}"},
        {"key":"station","value":"Bromo / Ijen"},{"key":"paging","value":"2"},
        {"key":"scope","value":"environment"}]}
    collection={"id":"api","name":"Fieldnotes API","environmentId":"local",
        "auth":{"type":"bearer","bearer":{"token":"fixture"}},
        "variables":[{"key":"scope","value":"collection"}],
        "items":[{"kind":"folder","id":"weather","name":"Weather","auth":{"type":"inherit"},"variables":[{"key":"scope","value":"folder"}],"items":[request]}]}
    if args.sessions:
        collection["auth"]={"type":"oauth2","oauth2":{
            "grantType":"client_credentials","clientId":"smoke",
            "tokenUrl":f"http://127.0.0.1:{server.server_port}/token"}}
    data={"version":2,"settings":{"general":{"theme":"dark","restoreLastSession":True},
        "gpuiLayout":{"responseHeight":280}},
        "activeWorkspaceId":"main","workspaces":[{"id":"main","name":"Fieldnotes","collections":[collection],
        "environments":[environment],"tabs":[],"activeTabIndex":-1},
        {"id":"second","name":"Secondary","collections":[],"environments":[],"tabs":[]}]}
    path=directory/"workspace.json"
    path.write_text(json.dumps(data))
    env=dict(os.environ)
    env.pop("WAYLAND_DISPLAY",None)
    env.pop("WAYLAND_SOCKET",None)
    executable=root/"src-gpui/target/release/postlite-gpui"
    with subprocess.Popen([executable,"--write-workspace",str(path)],env=env) as app:
        try:
            for _ in range(200):
                try:
                    wid=xdo("search","--onlyvisible","--all","--pid",app.pid,"--name","^Postlite GPUI$").splitlines()[0]
                    break
                except subprocess.CalledProcessError:
                    if app.poll() is not None:
                        raise RuntimeError(f"App exited: {app.returncode}")
                    time.sleep(.05)
            else:
                raise RuntimeError("No GPUI window")
            print(wid,directory,xdo("getwindowgeometry",wid),flush=True)
            xdo("windowactivate","--sync",wid)
            time.sleep(.4)
            capture(wid, "before.png")
            xdo("mousemove","--window",wid,90,147,"click",1)
            xdo("mousemove","--window",wid,100,174,"click",1)
            xdo("mousemove","--window",wid,140,201,"click",1)
            geometry=dict(part.split("=",1) for part in xdo("getwindowgeometry","--shell",wid).splitlines())
            height=int(geometry["HEIGHT"])
            width=int(geometry["WIDTH"])
            xdo("mousemove","--window",wid,width-247,123,"click",1)
            for _ in range(60):
                if sent:
                    break
                time.sleep(.05)
            expected=("/Bromo%20%2F%20Ijen?page=2","folder","Bearer fixture",'{"station": "Bromo / Ijen"}')
            assert sent==[(*expected,None)],sent
            time.sleep(.4)
            capture(wid, "sent.png")
            xdo("mousemove","--window",wid,width-100,height-288,"click",1)
            time.sleep(.15)
            copied=clipboard()
            assert copied == '{"gpui": "native saved request"}',copied
            if args.sessions:
                xdo("mousemove","--window",wid,width-96,175,"click",1)
                time.sleep(.5)
                capture(wid, "cookies.png")
                xdo("mousemove","--window",wid,width-24,55,"click",1)
                time.sleep(.25)
                xdo("mousemove","--window",wid,545,123,"click",1)
                xdo("key","ctrl+Return")
                for _ in range(60):
                    if len(sent) == 2:
                        break
                    time.sleep(.05)
                assert sent==[(*expected,None),(*expected,"fixture=remembered")],sent
                xdo("mousemove","--window",wid,width-96,175,"click",1)
                time.sleep(.5)
                xdo("mousemove","--window",wid,width-44,171,"click",1)
                time.sleep(.4)
                capture(wid, "cookies-deleted.png")
                xdo("mousemove","--window",wid,width-24,55,"click",1)
                time.sleep(.25)
                xdo("mousemove","--window",wid,545,123,"click",1)
                xdo("key","ctrl+Return")
                for _ in range(60):
                    if len(sent) == 3:
                        break
                    time.sleep(.05)
                assert sent==[(*expected,None),(*expected,"fixture=remembered"),(*expected,None)],sent
                assert tokens==["grant_type=client_credentials&client_id=smoke"],tokens
                print("PASS inherited OAuth token exchange and cache reuse, workspace cookie replay, and cookie drawer deletion",flush=True)
            xdo("mousemove","--window",wid,22,120,"click",1)
            xdo("mousemove","--window",wid,104,145,"click",1)
            time.sleep(.9)
            persisted=json.loads(path.read_text())
            active=persisted["workspaces"][0]
            assert active["tabs"][active["activeTabIndex"]]["envId"]=="local",active["tabs"]
            assert active["history"][0]["ref"] == "request"
            assert active["history"][0]["url"] == "{{base_url}}/:id"
            capture(wid, "environment.png")
            print("PASS nested request send, encoded path/query, environment override, folder override, inherited auth, and environment tab save",flush=True)
            if args.preview:
                xdo("mousemove","--window",wid,442,70,"click",1)
                xdo("mousemove","--window",wid,545,123,"click",1)
                xdo("key","ctrl+a")
                xdo("type",f"http://127.0.0.1:{server.server_port}/preview")
                xdo("key","ctrl+Return")
                xdo("mousemove","--window",wid,482,height-247,"click",1)
                time.sleep(.8)
                capture(wid, "preview.png")
                xdo("mousemove","--window",wid,339,height-205,"mousedown",1)
                time.sleep(.10)
                xdo("mousemove","--window",wid,550,height-180)
                time.sleep(.10)
                xdo("mouseup",1)
                xdo("key","ctrl+c")
                time.sleep(.10)
                copied=clipboard()
                assert copied.strip() == "Preview fixture",copied
                print("PASS formatted HTML preview and selection",flush=True)
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=4)

profile.cleanup()
