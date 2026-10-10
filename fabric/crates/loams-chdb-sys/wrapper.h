/* The translation unit bindgen runs over, per FL2 Ruling 1.
 *
 * Ruling 1 binds the header the pinned libchdb.so was built from and never
 * hand-writes a prototype: FL2 Task 0 drove the library through guessed 1.x-style
 * ctypes bindings (`chdb_connect(const char * path)`, a `char **error`
 * out-parameter) and segfaulted, because the real ABI is
 * `chdb_connect(int argc, char ** argv)` with no error out-parameter
 * (fl2-dependency-spike.md §3).
 *
 * So this header adds nothing to chdb.h and is here only to give bindgen one
 * file to open. `chdb.h` is vendored beside it from chdb-io/chdb-core at tag
 * v26.9.0 (programs/local/chdb.h) because the release tarball ships libchdb.so
 * and no header, so the header and the binary would otherwise drift apart.
 * chdb.h already includes <stddef.h> and <stdint.h> for size_t and uint64_t,
 * which is all bindgen needs beyond the declarations themselves.
 */
#include "chdb.h"