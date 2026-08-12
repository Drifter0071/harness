import struct, os, io

width, height = 32, 32

# Build rows as BGRA pixels
rows = []
for y in range(height):
    row = []
    for x in range(width):
        row.extend([0, 0, 0, 255])  # BGRA, black
    rows.append(row)

# Border (1px zinc-200)
for x in range(width):
    for ch in [231, 228, 228, 255]:  # BGR
        rows[0][x*4 + [231,228,228,255].index(ch)] = ch
        rows[31][x*4 + [231,228,228,255].index(ch)] = ch

# Simpler approach: build pixel data directly
pixels = bytearray(width * height * 4)

def set_pixel(px, py, r, g, b, a=255):
    if 0 <= px < width and 0 <= py < height:
        idx = (py * width + px) * 4
        pixels[idx] = b
        pixels[idx+1] = g
        pixels[idx+2] = r
        pixels[idx+3] = a

# Fill with black
for y in range(height):
    for x in range(width):
        set_pixel(x, y, 0, 0, 0)

# Border
for x in range(width):
    set_pixel(x, 0, 228, 228, 231)
    set_pixel(x, 31, 228, 228, 231)
for y in range(height):
    set_pixel(0, y, 228, 228, 231)
    set_pixel(31, y, 228, 228, 231)

# H letter
for y in range(10, 23):
    set_pixel(10, y, 228, 228, 231)
    set_pixel(11, y, 228, 228, 231)
    set_pixel(21, y, 228, 228, 231)
    set_pixel(22, y, 228, 228, 231)
for x in range(10, 23):
    set_pixel(x, 15, 228, 228, 231)
    set_pixel(x, 16, 228, 228, 231)

# Row size must be multiple of 4 bytes
row_size = width * 4
padded_row = (row_size + 3) & ~3
pixel_data_size = padded_row * height

# Build BMP data: bottom-up
bmp = bytearray()
for y in range(height - 1, -1, -1):
    row_start = y * width * 4
    row_end = row_start + row_size
    bmp.extend(pixels[row_start:row_end])
    bmp.extend(b'\x00' * (padded_row - row_size))

# AND mask (1 bit per pixel, all zeros = fully opaque via alpha)
and_mask_row = (width + 31) // 32 * 4
and_mask = bytearray(and_mask_row * height)  # all zeros

# BITMAPINFOHEADER
bih = struct.pack('<IiiHHIIiiII',
    40,           # header size
    width,        # width
    height * 2,   # height (doubled for ICO: XOR + AND mask)
    1,            # color planes
    32,           # bits per pixel
    0,            # compression (BI_RGB)
    len(bmp) + len(and_mask),  # image size
    0, 0,         # resolution
    0,            # colors in palette
    0             # important colors
)

image_data = bytes(bih) + bytes(bmp) + bytes(and_mask)

# ICO header
ico = struct.pack('<HHH', 0, 1, 1)  # reserved, type=ICO, count=1

# Directory entry
offset = 6 + 16  # header + 1 entry
ico += struct.pack('<BBBBHHIH',
    width, height, 0, 0, 1, 32, len(image_data), offset
)

ico += image_data

out = r'C:\Users\garvangel\Documents\GitHub\harness\src-tauri\icons\icon.ico'
with open(out, 'wb') as f:
    f.write(ico)

print(f'icon.ico: {os.path.getsize(out)} bytes')
