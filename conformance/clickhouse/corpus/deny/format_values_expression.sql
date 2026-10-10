-- expect: 344
SELECT * FROM format(Values, 'a String', $$(file('/etc/hostname'))$$)
