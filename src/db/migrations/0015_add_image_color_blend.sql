ALTER TABLE images ADD COLUMN color_blend REAL;  -- Auto Correct blend fraction, 0.0-1.0, NULL meaning never auto-corrected
UPDATE images SET color_blend = 1.0 WHERE color_black_r IS NOT NULL;  -- preserve existing corrections' appearance at full strength
