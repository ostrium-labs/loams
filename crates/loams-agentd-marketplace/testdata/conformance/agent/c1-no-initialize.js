'use strict';
// Fails LAPC-C1: never answers initialize.
require('./lib.js').run({ noInitialize: true });
