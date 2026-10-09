-- expect: 344
SELECT * FROM executable('cat', TSV, 'a String')
