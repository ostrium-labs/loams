'use strict';
// Fails LAPC-C7: ignores cancel and SIGTERM, and leaves a child process.
require('./lib.js').run({ hang: true });
