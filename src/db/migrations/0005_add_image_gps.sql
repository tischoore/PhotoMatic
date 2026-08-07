ALTER TABLE images ADD COLUMN gps_latitude REAL;   -- decimal degrees (WGS84), signed (N/E positive), nullable
ALTER TABLE images ADD COLUMN gps_longitude REAL;  -- decimal degrees (WGS84), signed (N/E positive), nullable
ALTER TABLE images ADD COLUMN gps_altitude REAL;   -- meters, signed (below sea level negative), nullable
