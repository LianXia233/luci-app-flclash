/* View registry: each js/views/*.js file registers one builder here. The
 * shell (app.js) calls FlClashViews[name] and receives a DOM node. */
window.FlClashViews = window.FlClashViews || {};
(function () {
  'use strict';
  const listeners = {};

  window.FlClashViews.emit = function (name) {
    (listeners[name] || []).forEach(function (fn) {
      try { fn(); } catch (error) { /* view may have been unmounted */ }
    });
  };

  window.FlClashViews.on = function (name, fn) {
    (listeners[name] = listeners[name] || []).push(fn);
    return function () {
      listeners[name] = (listeners[name] || []).filter(function (item) { return item !== fn; });
    };
  };
})();
