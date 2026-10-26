(function (w, d) {
  var s = d.currentScript;
  var days = +(s && s.getAttribute("data-days")) || 30;
  var endpoint = s && s.src ? new URL(s.src).origin + "/c" : "";
  var q = new URLSearchParams(w.location.search);

  function get(name) {
    var m = ("; " + d.cookie).split("; " + name + "=");
    return m.length > 1 ? decodeURIComponent(m.pop().split(";")[0]) : "";
  }

  function set(name, value, ttlDays) {
    d.cookie =
      name + "=" + encodeURIComponent(value) +
      ";max-age=" + Math.round(ttlDays * 86400) +
      ";path=/;SameSite=Lax" +
      (w.location.protocol === "https:" ? ";Secure" : "");
  }

  function uid() {
    var c = w.crypto;
    if (c && c.randomUUID) return c.randomUUID();
    var b = c.getRandomValues(new Uint8Array(16));
    return Array.prototype.map
      .call(b, function (x) { return (x + 256).toString(16).slice(1); })
      .join("");
  }

  function track(code) {
    var vid = get("bf_vid");
    if (!vid) set("bf_vid", (vid = uid()), 365);
    code = code || get("bf_ref");
    if (!code || !endpoint) return;
    set("bf_ref", code, days);
    var body = JSON.stringify({
      code: code,
      vid: vid,
      url: w.location.href,
      ref: d.referrer
    });
    var n = w.navigator;
    if (n.sendBeacon && n.sendBeacon(endpoint, new Blob([body], { type: "application/json" }))) return;
    w.fetch(endpoint, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: body,
      keepalive: true
    }).catch(function () {});
  }

  w.bunfiiliate = { track: track };
  var landed = q.get("ref") || q.get("bf_click");
  if (landed) track(landed);
  else if (!get("bf_vid")) set("bf_vid", uid(), 365);
})(window, document);
