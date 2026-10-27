'use strict';
// Fails LAPC-C4: asks to run a tool its listing does not declare.
require('./lib.js').run({ undeclaredTool: true });
