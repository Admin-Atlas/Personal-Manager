// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The harness plays PM's main window. It mounts each probe frame the way ModFrame mounts a mod
// (src/components/mods/ModFrame.tsx): the one-time entry URL Rust mints, ModFrame's `sandbox` and
// `allow` (FRAME_SANDBOX and FRAME_ALLOW below, which the app's build.rs holds equal to ModFrame's
// literals), and a MessagePort handed only to that frame's own connect request while its origin is
// opaque (modBridge.ts). It collects what the probe reports, measures whether a busy frame freezes
// this page, and has fresh frames try to load other pages: each navigation target gets a frame of
// its own, since a navigation an engine refuses can still replace the frame's document with an error
// page, and the probe would then be gone for the next target.
//
// Unlike ModFrame it keeps listening after a second `load`: WebKit fires one when a download is
// refused, with the probe's document still in place, and the probe has more to report. ModFrame's
// kill on that load is its own unit test's to prove.
//
// Everything the frame claims is a hint; the Rust side counts the real escapes and leaks. This page
// itself also proves that PM's top-frame script leaves PM's own origin alone: if it stopped this page,
// nothing would ever report and the watchdog would fail the run.

(function () {
  "use strict";
  // ModFrame's iframe attributes, exactly (build.rs compares them with ModFrame.tsx).
  var FRAME_SANDBOX = "allow-scripts";
  var FRAME_ALLOW = "";
  // Where each navigation frame tries to go (probe.js's TARGETS): its own entry page and the strict
  // frame's, both without a token; a data: and a blob: document that load a script from the mod's
  // own folder; a host Rust's URL parser refuses, so Tauri's runtime lets the navigation past the
  // guard unasked where the engine accepts the host (WebView2, Fedora's WebKitGTK 2.54; the
  // WebKitGTK on ubuntu-latest and the WebKit on macos-latest refuse it as an invalid URL, so there
  // it goes nowhere); and plain loopback.
  var NAV_TARGETS = ["own-entry", "other-entry", "data-doc", "blob-doc", "idna-host", "loopback"];
  var T = window.__TAURI_INTERNALS__;
  var status = document.getElementById("status");
  var say = function (s) {
    status.textContent = s;
  };
  var sleep = function (ms) {
    return new Promise(function (r) {
      setTimeout(r, ms);
    });
  };

  function mountFrame(entryUrl, variant) {
    var run = {
      variant: variant,
      loads: 0,
      loadTimes: [],
      connected: false,
      connectRequests: 0,
      refusedOrigins: [],
      results: [],
      done: false,
      busyDone: false,
      port: null,
    };
    var finish;
    var finished = new Promise(function (r) {
      finish = r;
    });
    var gotPort;
    var connected = new Promise(function (r) {
      gotPort = r;
    });
    var frame = document.createElement("iframe");
    frame.setAttribute("sandbox", FRAME_SANDBOX);
    frame.setAttribute("allow", FRAME_ALLOW);
    frame.setAttribute("src", entryUrl);
    frame.width = "800";
    frame.height = "200";

    var onMessage = function (ev) {
      if (ev.source === null || ev.source !== frame.contentWindow) return;
      // Every sealed frame is opaque. ModFrame refuses anything else, so a frame that isn't would
      // never get its port and the run fails as unfinished; the origin is kept for the report.
      if (ev.origin !== "null") {
        run.refusedOrigins.push(String(ev.origin));
        return;
      }
      var d = ev.data || {};
      if (d.pm !== "connect" || d.version !== 1) return;
      run.connectRequests += 1;
      if (run.port) return;
      var ch = new MessageChannel();
      run.port = ch.port1;
      run.connected = true;
      ch.port1.onmessage = function (e) {
        var m = e.data || {};
        if (m.type === "result") run.results.push(m.result);
        if (m.type === "done") {
          run.done = true;
          finish();
        }
        if (m.type === "busy-done") run.busyDone = true;
        // The relaxed frame's marks around its plain loads: Rust notes the sinkhole's count, and
        // only then does the frame hear back (and, at `open-begin`, fire its first load).
        if (m.type === "mark" && typeof m.name === "string") {
          var name = m.name;
          T.invoke("harness_mark", { variant: variant, mark: name }).then(
            function () {
              ch.port1.postMessage({ type: "marked", name: name, noted: true });
            },
            function (err) {
              ch.port1.postMessage({ type: "marked", name: name, noted: String(err) });
            },
          );
        }
      };
      // The target must be "*": the frame's origin is opaque and can't be named.
      frame.contentWindow.postMessage({ pm: "port", version: 1 }, "*", [ch.port2]);
      gotPort();
    };
    window.addEventListener("message", onMessage);
    frame.addEventListener("load", function () {
      run.loads += 1;
      run.loadTimes.push(Date.now());
    });
    document.getElementById("frames").appendChild(frame);
    var unmount = function () {
      window.removeEventListener("message", onMessage);
      if (run.port) run.port.close();
      frame.remove();
    };
    return { run: run, finished: finished, connected: connected, unmount: unmount };
  }

  // One navigation target in a fresh `nav` frame: once the probe has its port (so its scripts ran),
  // it navigates itself, and the loads that follow are counted. Rust counts what matters: any leak,
  // and any script the new document loads.
  async function navigationTest(to) {
    var entryUrl = await T.invoke("harness_open", { variant: "nav" });
    var mounted = mountFrame(entryUrl, "nav");
    var run = mounted.run;
    await Promise.race([mounted.connected, sleep(15000)]);
    var result = { to: to, connected: run.connected, loadsBefore: run.loads };
    if (run.port) {
      run.port.postMessage({ type: "navigate", to: to });
      await sleep(3000);
    }
    result.loadsAfter = run.loads;
    result.results = run.results;
    mounted.unmount();
    return result;
  }

  async function busyTest(run) {
    // The frame spins for 2 s while this page ticks every 50 ms; a gap near 2 s means the frame shares
    // this page's thread, so a frozen mod would freeze PM.
    var last = performance.now();
    var maxGap = 0;
    var tick = setInterval(function () {
      var now = performance.now();
      maxGap = Math.max(maxGap, now - last);
      last = now;
    }, 50);
    run.port.postMessage({ type: "busy" });
    await sleep(3500);
    clearInterval(tick);
    return { maxGapMs: Math.round(maxGap), frameReportedDone: run.busyDone };
  }

  async function main() {
    var report = {
      startedAt: new Date().toISOString(),
      userAgent: navigator.userAgent,
      pageOrigin: String(self.origin),
      runs: [],
    };
    try {
      for (var variant of ["strict", "relaxed"]) {
        say("running " + variant);
        var entryUrl = await T.invoke("harness_open", { variant: variant });
        var mounted = mountFrame(entryUrl, variant);
        var run = mounted.run;
        await Promise.race([mounted.finished, sleep(45000)]);
        var entry = {
          variant: variant,
          loads: run.loads,
          loadTimes: run.loadTimes,
          connected: run.connected,
          connectRequests: run.connectRequests,
          refusedOrigins: run.refusedOrigins,
          probeFinished: run.done,
          results: run.results,
        };
        if (variant === "strict" && run.port) {
          entry.busy = await busyTest(run);
        }
        report.runs.push(entry);
        mounted.unmount();
      }
      report.navigations = [];
      for (var to of NAV_TARGETS) {
        say("navigating to " + to);
        report.navigations.push(await navigationTest(to));
      }
    } catch (e) {
      report.harnessError = String((e && e.stack) || e);
    }
    say("reporting");
    await T.invoke("harness_report", { report: report });
    say("reported");
  }

  main();
})();
