"""Runs Tidis's Python suite (test/, unmodified) against a server, without
its Lua cases (KV1 plan KV0 Task 10, step 2; Q1223 leaves Lua out of v1).

Usage: python tidis_suite.py <tidis checkout> <host> <port>

This driver is ours: it imports Tidis's test cases and builds the suite
test_helper.py builds, minus LuaTest, with the server set directly instead
of through test_helper.py's arguments. Before each test it sends RESET on
the shared connection, so a test that leaves it inside MULTI on a server
whose MULTI semantics differ from Tidis's (test_multi_error on Valkey)
does not fail every test after it.
"""

import sys
import unittest


def main() -> int:
    checkout, host, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
    sys.path.insert(0, checkout + "/test")
    sys.argv = sys.argv[:1]
    from rediswrap import RedisWrapper

    RedisWrapper.set_instance_manually(host, port)
    import test_generic, test_hash, test_invalid, test_list, test_set, test_string, test_zset

    suite = unittest.TestSuite()
    loader = unittest.TestLoader()
    for case in (
        test_generic.GenericTest,
        test_invalid.InvalidTest,
        test_string.StringTest,
        test_hash.HashTest,
        test_list.ListTest,
        test_set.SetTest,
        test_zset.ZsetTest,
    ):
        suite.addTest(loader.loadTestsFromTestCase(case))
    redis = RedisWrapper.get_instance()

    class Resetting(unittest.TextTestResult):
        def startTest(self, test):
            try:
                redis.execute_command("RESET")
            except Exception as e:  # the server may not know RESET
                print(f"reset before {test.id()} failed: {e}")
            super().startTest(test)

    result = unittest.TextTestRunner(verbosity=2, resultclass=Resetting).run(suite)
    print(
        f"tidis suite: run {result.testsRun}, failures {len(result.failures)}, "
        f"errors {len(result.errors)}, skipped {len(result.skipped)}"
    )
    for test, _ in result.failures + result.errors:
        print(f"deviation: {test.id()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
