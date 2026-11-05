// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A hostile mod. It asks PM for its port the way a mod does (modBridge.ts), then tries every way out
// of the sealed frame we know of and says what happened. What it says is a hint, and a few of its
// answers are checked (an opaque origin, the WebRTC script ran, the parent's DOM blocked); the Rust
// side counts the real escapes (calls reaching the `canary` command) and leaks (connections to its
// loopback listeners), where the frame can't fake them.
//
// The `strict` frame is served with PM's real headers, whose CSP refuses every load it can name, so
// its network probes test the CSP, and the engine's wall below it only for what the CSP can't stop
// (preconnect, prefetch). It ends by reporting every refusal its CSP reported (`csp-violations`),
// which must name a load to the leak listener: the proof the reports arrive on this engine. The
// `relaxed` frame's CSP lets loads through, so after the IPC probes it fires plain loads
// (OPEN_NET_PROBES) at the leak listener and at the IDNA host: there they meet the sinkhole (Linux,
// Windows) or the content rule list (macOS), and one that gets through is a hit on the listener. A
// load to a host this engine's URL parser refuses fails before any of that, as the IDNA host's do
// on ubuntu-latest's WebKitGTK and macos-latest's WebKit, so first it reports which hosts parse
// (`open-parse`). It also reports every CSP refusal of those loads, which must be none, or the CSP
// is what was measured, and brackets them with two marks, at which Rust notes the sinkhole's
// count. A `nav` frame (PM's headers) runs no probes and only navigates itself to the one target
// the harness names.
//
// PM_PROBE comes from config.js, which the app writes for each run: the variant, the mod id, the
// loopback ports and the two origins.

(function () {
  "use strict";
  var CFG = self.PM_PROBE;
  var port = null;

  var sleep = function (ms) {
    return new Promise(function (r) {
      setTimeout(r, ms);
    });
  };
  var errName = function (e) {
    return String((e && e.name) || "Error") + ": " + String((e && e.message) || e);
  };
  var settle = function (p, ms) {
    return Promise.race([
      Promise.resolve(p).then(
        function (v) {
          return { ok: true, value: String(v) };
        },
        function (e) {
          return { ok: false, error: errName(e) };
        },
      ),
      sleep(ms).then(function () {
        return { ok: false, error: "timeout" };
      }),
    ]);
  };
  var leakBase = "http://127.0.0.1:" + CFG.tcpPort + "/leak/" + CFG.variant;
  var leak = function (name) {
    return leakBase + "/" + name;
  };
  // The CSP's refusals, as the document reports them (blocked URL and directive), from the start.
  var violations = [];
  document.addEventListener(
    "securitypolicyviolation",
    function (e) {
      violations.push({
        blocked: String(e.blockedURI),
        directive: String(e.effectiveDirective || e.violatedDirective),
      });
    },
    true,
  );
  // A mark around the relaxed frame's plain loads: the harness has Rust note the sinkhole's count,
  // then answers. Waited for, so no load starts before `open-begin` is noted. With no answer the
  // run goes on, and Rust, missing the mark, fails it on Linux.
  var answers = {};
  var mark = function (name) {
    return new Promise(function (res) {
      answers[name] = res;
      port.postMessage({ type: "mark", name: name });
      setTimeout(function () {
        res("no answer");
      }, 5000);
    });
  };
  var ownFolder = CFG.modOrigin + "/" + CFG.id + "/app/";
  var forged = function (tag) {
    return {
      cmd: "canary",
      callback: 4242,
      error: 4243,
      payload: { via: CFG.variant + ":" + tag },
      options: {},
      __TAURI_INVOKE_KEY__: "guess",
    };
  };
  var ipcHeaders = {
    "Content-Type": "application/json",
    "Tauri-Callback": "4242",
    "Tauri-Error": "4243",
    "Tauri-Invoke-Key": "guess",
  };
  var linkProbe = function (rel, href) {
    return new Promise(function (res) {
      try {
        var l = document.createElement("link");
        l.rel = rel;
        l.href = href;
        l.onload = function () {
          res("LOADED");
        };
        l.onerror = function () {
          res("error");
        };
        document.head.appendChild(l);
        setTimeout(function () {
          res("added; no load or error event");
        }, 3000);
      } catch (e) {
        res("threw " + errName(e));
      }
    });
  };

  // One fetch, image or WebSocket to `url`, and what came of it within `ms`.
  var fetchProbe = function (url, ms) {
    return settle(
      fetch(url).then(function (r) {
        return "status " + r.status;
      }),
      ms,
    );
  };
  var imgProbe = function (url, ms) {
    return new Promise(function (res) {
      var i = new Image();
      i.onload = function () {
        res("LOADED");
      };
      i.onerror = function () {
        res("error");
      };
      i.src = url;
      setTimeout(function () {
        res("timeout");
      }, ms);
    });
  };
  var wsProbe = function (url, ms) {
    return new Promise(function (res) {
      try {
        var ws = new WebSocket(url);
        ws.onopen = function () {
          res("OPEN");
        };
        ws.onerror = function () {
          res("error");
        };
      } catch (e) {
        res("threw " + errName(e));
      }
      setTimeout(function () {
        res("timeout");
      }, ms);
    });
  };

  function webrtcProbe(w) {
    var RTC = w.RTCPeerConnection || w.webkitRTCPeerConnection;
    if (!RTC) return Promise.resolve("absent");
    return (async function () {
      try {
        var pc = new RTC({ iceServers: [{ urls: "stun:127.0.0.1:" + CFG.udpPort }] });
        pc.createDataChannel("x");
        var offer = await pc.createOffer();
        await pc.setLocalDescription(offer);
        await sleep(2500);
        var state = pc.iceGatheringState;
        pc.close();
        return "constructed; ice gathering " + state;
      } catch (e) {
        return "threw " + errName(e);
      }
    })();
  }

  var IPC_PROBES = [
    [
      "origin",
      function () {
        return { origin: self.origin, href: location.href, secure: self.isSecureContext };
      },
    ],
    [
      "globals",
      function () {
        var handlers = window.webkit && window.webkit.messageHandlers;
        return {
          pmPreamble: self.__pmPreamble === true,
          tauriInternals: typeof window.__TAURI_INTERNALS__,
          tauri: typeof window.__TAURI__,
          windowIpc: typeof window.ipc,
          chromeWebview: typeof (window.chrome && window.chrome.webview),
          webkitHandlers: handlers ? Object.keys(handlers) : null,
          webkitIpc: typeof (handlers && handlers.ipc),
          internalsKeys: window.__TAURI_INTERNALS__
            ? Object.keys(window.__TAURI_INTERNALS__)
            : null,
        };
      },
    ],
    [
      "invoke-internals",
      function () {
        var t = window.__TAURI_INTERNALS__;
        return t && t.invoke
          ? settle(t.invoke("canary", { via: CFG.variant + ":internals.invoke" }), 6000)
          : "absent";
      },
    ],
    [
      "fetch-ipc-http",
      function () {
        return settle(
          fetch("http://ipc.localhost/canary", {
            method: "POST",
            body: JSON.stringify({ via: CFG.variant + ":fetch http://ipc.localhost" }),
            headers: ipcHeaders,
          }).then(function (r) {
            return r.status + " " + r.headers.get("Tauri-Response");
          }),
          5000,
        );
      },
    ],
    [
      "fetch-ipc-scheme",
      function () {
        return settle(
          fetch("ipc://localhost/canary", {
            method: "POST",
            body: JSON.stringify({ via: CFG.variant + ":fetch ipc://localhost" }),
            headers: ipcHeaders,
          }).then(function (r) {
            return r.status + " " + r.headers.get("Tauri-Response");
          }),
          5000,
        );
      },
    ],
    [
      "postmessage-window-ipc",
      function () {
        if (!(window.ipc && window.ipc.postMessage)) return "absent";
        window.ipc.postMessage(JSON.stringify(forged("window.ipc.postMessage")));
        return "sent";
      },
    ],
    [
      "postmessage-webkit",
      function () {
        var h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc;
        if (!h) return "absent";
        h.postMessage(JSON.stringify(forged("webkit.messageHandlers.ipc")));
        return "sent";
      },
    ],
    [
      "postmessage-chrome-webview",
      function () {
        var w = window.chrome && window.chrome.webview;
        if (!w) return "absent";
        w.postMessage(JSON.stringify(forged("chrome.webview.postMessage")));
        return "sent";
      },
    ],
    [
      "parent-dom",
      function () {
        try {
          return "READ " + String(window.parent.document.title);
        } catch (e) {
          return "blocked " + errName(e);
        }
      },
    ],
    [
      "parent-tauri",
      function () {
        try {
          return "READ " + typeof window.parent.__TAURI_INTERNALS__;
        } catch (e) {
          return "blocked " + errName(e);
        }
      },
    ],
  ];

  var NET_PROBES = [
    [
      "storage",
      function () {
        var r = {};
        try {
          localStorage.setItem("x", "1");
          r.localStorage = "WORKS";
        } catch (e) {
          r.localStorage = "blocked " + errName(e);
        }
        try {
          document.cookie = "a=b";
          r.cookie = document.cookie ? "WORKS" : "empty";
        } catch (e) {
          r.cookie = "blocked " + errName(e);
        }
        try {
          indexedDB.open("x");
          r.indexedDB = "open() returned";
        } catch (e) {
          r.indexedDB = "blocked " + errName(e);
        }
        return r;
      },
    ],
    [
      "module-script",
      function () {
        return new Promise(function (res) {
          var s = document.createElement("script");
          s.type = "module";
          s.src = ownFolder + "module.js";
          s.onload = function () {
            res("loaded; ran " + (self.__pmModuleRan === true));
          };
          s.onerror = function () {
            res("error");
          };
          document.head.appendChild(s);
          setTimeout(function () {
            res("no load or error event");
          }, 4000);
        });
      },
    ],
    [
      "font-own",
      function () {
        return typeof FontFace === "function"
          ? settle(
              new FontFace("pmprobe", "url(" + ownFolder + "font.woff2)").load().then(function () {
                return "LOADED";
              }),
              4000,
            )
          : "absent";
      },
    ],
    [
      "fetch-loopback",
      function () {
        return fetchProbe(leak("fetch"), 4000);
      },
    ],
    [
      "xhr-loopback",
      function () {
        return new Promise(function (res) {
          try {
            var x = new XMLHttpRequest();
            x.open("GET", leak("xhr"));
            x.onload = function () {
              res("LOADED " + x.status);
            };
            x.onerror = function () {
              res("error");
            };
            x.send();
          } catch (e) {
            res("threw " + errName(e));
          }
          setTimeout(function () {
            res("timeout");
          }, 4000);
        });
      },
    ],
    [
      "websocket-loopback",
      function () {
        return wsProbe(leak("ws").replace("http:", "ws:"), 4000);
      },
    ],
    [
      "beacon-loopback",
      function () {
        try {
          return "returned " + navigator.sendBeacon(leak("beacon"), "x");
        } catch (e) {
          return "threw " + errName(e);
        }
      },
    ],
    [
      "img-loopback",
      function () {
        return imgProbe(leak("img"), 4000);
      },
    ],
    [
      "css-loopback",
      function () {
        return linkProbe("stylesheet", leak("css"));
      },
    ],
    [
      "prefetch-loopback",
      function () {
        return linkProbe("prefetch", leak("prefetch"));
      },
    ],
    // Preconnect opens TCP whatever the CSP says: the leak the sinkhole (Linux, Windows) and the
    // preference switches (macOS) exist to close. A loopback target makes a leak visible as a hit on
    // the app's listener; on Linux and Windows the sinkhole absorbs the attempt instead.
    [
      "preconnect-loopback",
      function () {
        return linkProbe("preconnect", "http://127.0.0.1:" + CFG.tcpPort);
      },
    ],
    [
      "preconnect-localhost",
      function () {
        return linkProbe("preconnect", "http://localhost:" + CFG.tcpPort);
      },
    ],
    [
      "preconnect-remote",
      function () {
        return linkProbe("preconnect", "http://pm-sealed-frame.invalid:81");
      },
    ],
    [
      "dns-prefetch",
      function () {
        return linkProbe("dns-prefetch", "//pm-sealed-frame-dns.invalid");
      },
    ],
    [
      "font-loopback",
      function () {
        return typeof FontFace === "function"
          ? settle(
              new FontFace("x", "url(" + leak("font") + ")").load().then(function () {
                return "LOADED";
              }),
              4000,
            )
          : "absent";
      },
    ],
    [
      "blob-worker",
      function () {
        return new Promise(function (res) {
          try {
            var src =
              "onmessage = async (e) => { const base = e.data; const out = {};" +
              " try { const r = await fetch(base + '/worker-fetch'); out.fetch = 'status ' + r.status; } catch (err) { out.fetch = 'blocked ' + err.name; }" +
              " try { importScripts(base + '/worker-import'); out.importScripts = 'LOADED'; } catch (err) { out.importScripts = 'blocked ' + err.name; }" +
              " try { new WebSocket(base.replace('http:', 'ws:') + '/worker-ws'); out.ws = 'constructed'; } catch (err) { out.ws = 'blocked ' + err.name; }" +
              " postMessage(out); };";
            var url = URL.createObjectURL(new Blob([src], { type: "text/javascript" }));
            var w = new Worker(url);
            w.onmessage = function (e) {
              res(Object.assign({ ran: true }, e.data));
            };
            w.onerror = function (e) {
              res({ ran: false, error: String((e && e.message) || "error") });
            };
            w.postMessage(leakBase);
          } catch (e) {
            res({ ran: false, threw: errName(e) });
          }
          setTimeout(function () {
            res({ ran: "timeout" });
          }, 6000);
        });
      },
    ],
    [
      "webrtc",
      function () {
        return webrtcProbe(window);
      },
    ],
    [
      "webrtc-via-nosrc-iframe",
      function () {
        var f = document.createElement("iframe");
        document.body.appendChild(f);
        var w = f.contentWindow;
        return w ? webrtcProbe(w) : "no contentWindow";
      },
    ],
    [
      "window-open",
      function () {
        try {
          var w = window.open(leak("open"));
          return w ? "OPENED" : "null";
        } catch (e) {
          return "threw " + errName(e);
        }
      },
    ],
    [
      "download-anchor",
      function () {
        var a = document.createElement("a");
        a.href = URL.createObjectURL(new Blob(["sealed-frame probe"]));
        a.download = "pm-sealed-frame-download.txt";
        document.body.appendChild(a);
        a.click();
        return "clicked";
      },
    ],
    [
      "form-submit",
      function () {
        var f = document.createElement("form");
        f.action = leak("form");
        f.method = "POST";
        document.body.appendChild(f);
        try {
          f.submit();
          return "submit() returned";
        } catch (e) {
          return "threw " + errName(e);
        }
      },
    ],
    [
      "eval",
      function () {
        try {
          return "RAN " + (0, eval)("1+1");
        } catch (e) {
          return "blocked " + errName(e);
        }
      },
    ],
    [
      "top-navigation",
      function () {
        try {
          window.top.location.href = ownFolder + "index.html#top";
          return "no throw";
        } catch (e) {
          return "blocked " + errName(e);
        }
      },
    ],
    // Every refusal this frame's CSP reported. Rust needs one for a load to the leak listener: the
    // proof that this engine's violation reports arrive at all, without which the relaxed frame's
    // empty `open-csp` would mean nothing.
    [
      "csp-violations",
      function () {
        return sleep(500).then(function () {
          return violations.slice();
        });
      },
    ],
  ];

  // The `relaxed` frame's plain loads (main.rs `OPEN_LOADS`): a fetch, an image and a WebSocket, each
  // in the clear and over TLS, each to the leak listener and to the IDNA host (mapped to 127.0.0.1
  // in CI, so a load that gets through is a hit there too). Its CSP lets them go, so the engine's
  // network wall is what has to stop them; `open-csp` then lists any refusal of them by the CSP,
  // which must be none. The TLS ones are what no other rule can stop first: a page in a secure
  // context may refuse a load in the clear as mixed content, and macOS's App Transport Security a
  // named host in the clear, but neither refuses `https:` or `wss:`. Their TCP connection is a hit
  // before any handshake. `open-begin` and `open-end` are the marks around them: the sinkhole's count
  // between the two is theirs alone, not the IPC probes' (whose `http://ipc.localhost` fetch the
  // relaxed CSP allows, and which reaches the proxy too).
  var OPEN_KINDS = [
    ["fetch", "http", "https", fetchProbe],
    ["img", "http", "https", imgProbe],
    ["ws", "ws", "wss", wsProbe],
  ];
  var OPEN_NET_PROBES = [
    // Which of the two hosts this engine's URL parser takes. A load to a URL it refuses fails
    // before any network, so it never reaches the wall below: the sinkhole's count is owed one
    // connection per load to a host this lists, and none for the rest. It loads nothing, and runs
    // before `open-begin`.
    [
      "open-parse",
      function () {
        var parses = function (host) {
          try {
            new URL("http://" + host + ":" + CFG.tcpPort + "/");
            return true;
          } catch {
            return false;
          }
        };
        return { loopback: parses("127.0.0.1"), idna: parses(CFG.idnaHost) };
      },
    ],
    [
      "open-begin",
      function () {
        return mark("open-begin").then(function (noted) {
          // Only refusals of the loads below count.
          violations.length = 0;
          return { secureContext: self.isSecureContext === true, mark: noted };
        });
      },
    ],
  ];
  ["loopback", "idna"].forEach(function (target) {
    var host = target === "idna" ? CFG.idnaHost : "127.0.0.1";
    OPEN_KINDS.forEach(function (kind) {
      [false, true].forEach(function (tls) {
        var name = "open-" + kind[0] + (tls ? "-tls-" : "-") + target;
        var url =
          (tls ? kind[2] : kind[1]) +
          "://" +
          host +
          ":" +
          CFG.tcpPort +
          "/leak/" +
          CFG.variant +
          "/" +
          name;
        OPEN_NET_PROBES.push([
          name,
          function () {
            return kind[3](url, 3000);
          },
        ]);
      });
    });
  });
  OPEN_NET_PROBES.push(
    [
      "open-csp",
      function () {
        // A refusal is reported in a task of its own; give the last one a moment to arrive.
        return sleep(500).then(function () {
          return violations.slice();
        });
      },
    ],
    // After that moment, so a load's late connection is still inside the marks.
    [
      "open-end",
      function () {
        return mark("open-end");
      },
    ],
  );

  // Probes that might navigate or reload the frame get a pause after them, so the harness's load
  // timestamps say which one caused a load.
  var SPACED = ["webrtc-via-nosrc-iframe", "window-open", "download-anchor", "form-submit"];

  async function runAll() {
    // Only `strict` and `relaxed` run probes (a `nav` frame only navigates).
    var list = IPC_PROBES.concat(CFG.variant === "strict" ? NET_PROBES : OPEN_NET_PROBES);
    for (var i = 0; i < list.length; i++) {
      var name = list[i][0];
      var out;
      try {
        out = await list[i][1]();
      } catch (e) {
        out = "probe threw " + errName(e);
      }
      port.postMessage({ type: "result", result: { name: name, out: out, at: Date.now() } });
      if (SPACED.indexOf(name) !== -1) await sleep(1500);
    }
    port.postMessage({ type: "done" });
  }

  // A document of the probe's own making that loads probe-child.js from the mod's folder (which the
  // CSP it would inherit allows), so Rust sees whether it ran at all, and the child tries the network
  // and IPC from there. `via` names the target in the leak URLs.
  var childHtml = function (via) {
    var src =
      ownFolder + "probe-child.js?via=" + via + "&tcp=" + CFG.tcpPort + "&udp=" + CFG.udpPort;
    return '<!doctype html><p>child</p><script src="' + src + '"></script>';
  };

  // The harness's commands name a fixed target; none carries a URL. Each is built only when used, in
  // a frame of its own (harness.js).
  var TARGETS = {
    "own-entry": function () {
      return ownFolder + "index.html";
    },
    "other-entry": function () {
      return CFG.modOrigin + "/" + CFG.otherId + "/app/index.html";
    },
    // WebView2's guard never sees a frame, so main's frame-src is what refuses these there.
    "data-doc": function () {
      return "data:text/html," + encodeURIComponent(childHtml("data"));
    },
    "blob-doc": function () {
      return URL.createObjectURL(new Blob([childHtml("blob")], { type: "text/html" }));
    },
    // The `url` crate refuses this host, so where the engine accepts it (WebView2, Fedora's
    // WebKitGTK 2.54) tauri-runtime-wry allows the navigation without asking PM's guard. The
    // WebKitGTK on ubuntu-latest and the WebKit on macos-latest refuse it as an invalid URL, so
    // there it reaches neither the guard nor the network. CI maps it to 127.0.0.1, so a load is a
    // loopback hit.
    "idna-host": function () {
      return "http://" + CFG.idnaHost + ":" + CFG.tcpPort + "/leak/self-navigate-idna";
    },
    loopback: function () {
      return leak("self-navigate");
    },
  };

  function onCommand(ev) {
    var m = ev.data || {};
    if (m.type === "busy") {
      var end = performance.now() + 2000;
      while (performance.now() < end) {
        // spin
      }
      port.postMessage({ type: "busy-done" });
    } else if (
      m.type === "marked" &&
      typeof m.name === "string" &&
      Object.prototype.hasOwnProperty.call(answers, m.name)
    ) {
      answers[m.name](m.noted === true ? "noted" : "not noted: " + String(m.noted));
    } else if (m.type === "navigate" && Object.prototype.hasOwnProperty.call(TARGETS, m.to)) {
      try {
        location.href = TARGETS[m.to]();
      } catch (e) {
        port.postMessage({
          type: "result",
          result: { name: "navigate-" + m.to, out: "threw " + errName(e) },
        });
      }
    }
  }

  // The bridge's handshake, as a mod makes it: ask the parent for a port until it answers. Only PM's
  // page, the parent, may answer.
  window.addEventListener("message", function (ev) {
    if (ev.source !== window.parent || ev.origin !== CFG.appOrigin) return;
    var m = ev.data || {};
    if (m.pm === "port" && m.version === 1 && !port && ev.ports && ev.ports[0]) {
      port = ev.ports[0];
      port.onmessage = onCommand;
      // A `nav` frame only waits for its one navigation.
      if (CFG.variant !== "nav") runAll();
    }
  });
  var ask = function () {
    if (port) return;
    window.parent.postMessage({ pm: "connect", version: 1 }, "*");
    setTimeout(ask, 1000);
  };
  ask();
})();
