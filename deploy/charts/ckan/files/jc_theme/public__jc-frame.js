// A framed table view takes the height of its content (T-4637). Inside the frame the view says
// how tall its table is; the page that frames it sizes that frame. Same origin only, a number
// only; without script the frame keeps the height the stylesheet gives it.
(function () {
  var TYPE = "jc-frame-height";

  if (window.parent !== window) {
    var tries = 0;
    var watch = function () {
      var wrapper = document.getElementById("dtprv_wrapper");
      if (!wrapper) {
        // DataTables builds the wrapper after its first answer; ten seconds is a dead view.
        if (++tries < 50) setTimeout(watch, 200);
        return;
      }
      var say = function () {
        var height = Math.ceil(wrapper.getBoundingClientRect().bottom + window.scrollY);
        window.parent.postMessage({ type: TYPE, height: height }, window.location.origin);
      };
      if (window.ResizeObserver) new ResizeObserver(say).observe(wrapper);
      say();
    };
    watch();
    return;
  }

  window.addEventListener("message", function (event) {
    var data = event.data;
    if (event.origin !== window.location.origin || !data || data.type !== TYPE) return;
    var height = Number(data.height);
    if (!isFinite(height) || height <= 0) return;
    var frames = document.getElementsByTagName("iframe");
    for (var i = 0; i < frames.length; i++) {
      if (frames[i].contentWindow === event.source) {
        // ponytail: 10rem..80rem bounds a broken view; a longer table scrolls inside its frame.
        frames[i].style.height = Math.min(Math.max(height, 160), 1280) + "px";
        return;
      }
    }
  });
})();
