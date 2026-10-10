-- kind: CreatePipe
CREATE TABLE q (a String) ENGINE = LoamsStream('orders', 'JSONEachRow')
