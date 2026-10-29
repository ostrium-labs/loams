'use strict';
// Fails LAPC-C2: answers ACP protocol version 2 where the listing declares 1.
require('./lib.js').run({ protocolVersion: 2 });
