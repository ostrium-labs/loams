SELECT replaceRegexpAll('aaa', 'a', '\\0\\0'), replaceRegexpAll('abc', '(b)', '[\\1\\\\]'), replaceRegexpAll('a.b', '\\.', '$0'), replaceRegexpAll('abc', 'b', '\\x')
