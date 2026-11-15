(function (w, d) {
  try {
    var s = d.currentScript;
    var attr = function (k) { return (s && s.getAttribute(k)) || ""; };
    var days = +attr("data-days") || 90;
    var origin = s && s.src ? new URL(s.src).origin : "";
    var pk = attr("data-pk") || (s && s.src && new URL(s.src).searchParams.get("pk")) || "";
    var endpoint = origin + "/v2/c";
    var n = w.navigator;
    var dnt = s && s.getAttribute("data-respect-dnt") !== null && n && n.doNotTrack === "1";
    var ID = /^[A-Za-z0-9_-]{1,64}$/;

    var get = function (name) {
      var m = ("; " + d.cookie).split("; " + name + "=");
      return m.length > 1 ? decodeURIComponent(m.pop().split(";")[0]) : "";
    };
    var set = function (name, value, ttl) {
      d.cookie = name + "=" + encodeURIComponent(value) + ";max-age=" + Math.round(ttl * 86400) +
        ";path=/;SameSite=Lax" + (w.location.protocol === "https:" ? ";Secure" : "");
    };
    var store = function (k, v) {
      try {
        if (v) w.localStorage.setItem(k, v);
        else return w.localStorage.getItem(k) || "";
      } catch (e) { return ""; }
    };
    var uid = function () {
      var c = w.crypto, b = c.getRandomValues(new Uint8Array(16));
      return "vid_" + Array.prototype.map.call(b, function (x) { return (x + 256).toString(16).slice(1); }).join("");
    };
    var vid = function () {
      var v = store("bf_vid") || get("bf_vid");
      if (!/^[A-Za-z0-9_-]{8,64}$/.test(v)) v = uid();
      store("bf_vid", v);
      set("bf_vid", v, 365);
      return v;
    };

    var track = function (code, click, fresh) {
      try {
        if (dnt) return;
        var v = vid();
        if (!fresh) { code = code || get("bf_ref"); click = click || get("bf_click"); }
        if (!ID.test(code || "")) code = "";
        if (!ID.test(click || "")) click = "";
        if (!(code || click) || !origin) return;
        if (code) set("bf_ref", code, days);
        if (click) set("bf_click", click, days);
        var o = { v: 2, vid: v, url: w.location.origin + w.location.pathname, ts: Date.now() };
        if (pk) o.pk = pk;
        if (code) o.code = code;
        if (click) o.click = click;
        if (d.referrer) o.ref = new URL(d.referrer).origin;
        var body = JSON.stringify(o);
        if (n.sendBeacon && n.sendBeacon(endpoint, new Blob([body], { type: "text/plain" }))) return;
        w.fetch(endpoint, { method: "POST", headers: { "content-type": "text/plain" }, body: body, keepalive: true, credentials: "omit" })
          .catch(function () {});
      } catch (e) {}
    };

    w.bunfiiliate = { track: track, click_id: function () { return get("bf_click") || null; } };
    var q = new URLSearchParams(w.location.search), ref = q.get("ref"), click = q.get("bf_click");
    if (ref || click) track(ref, click, true);
    else if (!dnt) vid();
  } catch (e) {}
})(window, document);
