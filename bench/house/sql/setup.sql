CREATE TABLE IF NOT EXISTS kv (id {KV_INT}, v {KV_INT}, s String) ENGINE = MergeTree ORDER BY id
CREATE TABLE IF NOT EXISTS hits (WatchID UInt64, UserID UInt64, CounterID UInt32, RegionID UInt32, AdvEngineID UInt16, ResolutionWidth UInt16, IsRefresh UInt8, EventDate Date, EventTime DateTime, SearchPhrase String, URL String) ENGINE = MergeTree ORDER BY (CounterID, EventDate, UserID)
CREATE TABLE IF NOT EXISTS ins_p1 (id UInt64, ts DateTime, k UInt32, v Float64, s String) ENGINE = MergeTree ORDER BY (k, ts)
CREATE TABLE IF NOT EXISTS ins_p4 (id UInt64, ts DateTime, k UInt32, v Float64, s String) ENGINE = MergeTree PARTITION BY k % 4 ORDER BY (k, ts)
CREATE TABLE IF NOT EXISTS ins_p16 (id UInt64, ts DateTime, k UInt32, v Float64, s String) ENGINE = MergeTree PARTITION BY k % 16 ORDER BY (k, ts)
