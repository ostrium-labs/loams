'use strict';
// Fails LAPC-C6: writes into its own package, outside its data directory.
require('./lib.js').run({ writeOutside: true });
