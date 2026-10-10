-- kind: CreatePipe
CREATE TABLE q (a String) ENGINE = S3Queue('ns/1/in/*.csv', 'CSV')
