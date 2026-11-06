// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Removes WebRTC's constructors from every mod document before the mod's own scripts run (Mods spec
// §1.4, "What PR-1 ships" item 2). WebRTC's STUN probe goes out over UDP, which no CSP and no proxy
// stops: the step 0 spike measured it leaking from a sandboxed frame on macOS and Windows, and this
// script, injected into every frame, closing it on both.
//
// It acts only in a document whose origin is opaque (every mod frame: the sandbox) or whose URL is a
// mod's, so PM's own page keeps WebRTC. PM uses none of it today. getUserMedia is not WebRTC's to
// remove, so it stays everywhere; a mod frame's camera and microphone are refused by its
// Permissions-Policy instead (`headers.rs`).
//
// Each name is deleted, then pinned to `undefined` and made non-configurable, so the mod can't put
// one back. The marker lets the sealed-frame regression test see that the script ran in the frame.
(function () {
  try {
    var mod =
      String(self.origin) === "null" ||
      location.protocol === "pmmod:" ||
      location.hostname === "pmmod.localhost";
    if (!mod) return;
    var names = [
      "RTCPeerConnection",
      "webkitRTCPeerConnection",
      "RTCDataChannel",
      "RTCSessionDescription",
      "RTCIceCandidate",
      "RTCRtpSender",
      "RTCRtpReceiver",
      "RTCRtpTransceiver",
      "RTCDtlsTransport",
      "RTCIceTransport",
      "RTCSctpTransport",
      "RTCCertificate",
      "RTCEncodedAudioFrame",
      "RTCEncodedVideoFrame",
    ];
    for (var i = 0; i < names.length; i++) {
      // WebIDL makes an interface object a configurable property of the global, so both steps
      // succeed on every engine the spike measured. Each is tried on its own all the same: one
      // name an engine guards differently must not stop the rest being removed.
      try {
        delete self[names[i]];
      } catch {
        // The pin below still tries.
      }
      try {
        Object.defineProperty(self, names[i], {
          value: undefined,
          writable: false,
          configurable: false,
        });
      } catch {
        // Only reachable if the delete above failed too.
      }
    }
    try {
      Object.defineProperty(self, "__pmPreamble", { value: true });
    } catch {
      // Only the regression test reads the marker.
    }
  } catch {
    // Never let this script break a document: the frame's sandbox and CSP still stand.
  }
})();
