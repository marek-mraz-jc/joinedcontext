// T-4740: every "ask" button opens the docked assistant (jc-assistant.js places it). The chat shows
// the same six questions as its suggestions; it takes no question from the page.
(function () {
  var box = document.getElementById("jc-assistant");
  if (!box) {
    return;
  }
  Array.prototype.forEach.call(document.querySelectorAll("[data-ask]"), function (button) {
    button.addEventListener("click", function () {
      box.open = true;
      var frame = box.querySelector("iframe");
      if (frame) {
        frame.loading = "eager";
        frame.focus();
      }
    });
  });
})();
