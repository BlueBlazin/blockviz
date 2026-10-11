'use strict';
(function (root, factory) {
  if (typeof define === 'function' && define.amd) define([], factory);
  else if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.Lib = factory();
}(typeof self !== 'undefined' ? self : this, function () {
  var counter = 0;
  function next() { return ++counter; }
  return { next: next };
}));
var helper = require('./helper')
exports.run = function run(argv) { switch (argv[0]) { case 'a': case 'b': return 1; default: return helper(argv) } }
if (require.main === module) { exports.run(process.argv.slice(2)); }
