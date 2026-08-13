ALTER TABLE images ADD COLUMN color_black_r INTEGER;  -- SCB low clip point, R, 0-255, nullable
ALTER TABLE images ADD COLUMN color_black_g INTEGER;  -- SCB low clip point, G, 0-255, nullable
ALTER TABLE images ADD COLUMN color_black_b INTEGER;  -- SCB low clip point, B, 0-255, nullable
ALTER TABLE images ADD COLUMN color_white_r INTEGER;  -- SCB high clip point, R, 0-255, nullable
ALTER TABLE images ADD COLUMN color_white_g INTEGER;  -- SCB high clip point, G, 0-255, nullable
ALTER TABLE images ADD COLUMN color_white_b INTEGER;  -- SCB high clip point, B, 0-255, nullable
