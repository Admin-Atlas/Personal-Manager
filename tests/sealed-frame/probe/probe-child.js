// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The script a data: or blob: document loads after a sealed frame navigated itself there (probe.js's
// `data-doc` and `blob-doc` targets). That navigation should never happen: main's CSP frame-src names
// only the mod origins. So Rust fails the run as soon as this file is fetched at all. Should it run,
// it tries what a mod would want from such a document (WebRTC, preconnect, a fetch, an image, a
// forged invoke), so a leak is counted too. Its ports come from its own URL's query, which the
// scheme ignores.

(function () {
  "use strict";
  var q = new URL(document.currentScript.src).searchParams;
  var via = "child-" + q.get("via");
  var leak = "http://127.0.0.1:" + q.get("tcp") + "/leak/" + via + "/";
  var tries = [
    function () {
      var RTC = self.RTCPeerConnection || self.webkitRTCPeerConnection;
      if (!RTC) return;
      var pc = new RTC({ iceServers: [{ urls: "stun:127.0.0.1:" + q.get("udp") }] });
      pc.createDataChannel("x");
      pc.createOffer()
        .then(function (offer) {
          return pc.setLocalDescription(offer);
        })
        .catch(function () {});
    },
    function () {
      var l = document.createElement("link");
      l.rel = "preconnect";
      l.href = "http://127.0.0.1:" + q.get("tcp");
      document.head.appendChild(l);
    },
    function () {
      fetch(leak + "fetch").catch(function () {});
    },
    function () {
      new Image().src = leak + "img";
    },
    function () {
      var t = self.__TAURI_INTERNALS__;
      if (t && t.invoke) t.invoke("canary", { via: via }).catch(function () {});
    },
  ];
  for (var i = 0; i < tries.length; i++) {
    try {
      tries[i]();
    } catch {
      // Each try is on its own; a refusal is the expected answer.
    }
  }
})();
