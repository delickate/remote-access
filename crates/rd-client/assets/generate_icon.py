"""Generates the app logo/icon assets.

Run from this directory:  python generate_icon.py
Outputs:
  icon.png  - 256x256, used for the window/taskbar icon and the in-app logo
  icon.ico  - multi-size Windows icon embedded into the .exe
"""

from PIL import Image, ImageDraw

S = 1024  # draw large, then downscale for smooth edges

# Brand colours (match palette in src/main.rs)
TOP_LEFT = (129, 140, 248)    # indigo-400
BOTTOM_RIGHT = (109, 40, 217)  # violet-700
WHITE = (255, 255, 255, 255)
SCREEN = (30, 27, 75, 255)    # indigo-950, a dark "display"


def gradient(size):
    """Diagonal gradient from TOP_LEFT to BOTTOM_RIGHT."""
    small = Image.new("RGBA", (2, 2))
    small.putpixel((0, 0), TOP_LEFT + (255,))
    small.putpixel((1, 1), BOTTOM_RIGHT + (255,))
    mid = tuple((a + b) // 2 for a, b in zip(TOP_LEFT, BOTTOM_RIGHT)) + (255,)
    small.putpixel((1, 0), mid)
    small.putpixel((0, 1), mid)
    return small.resize((size, size), Image.BILINEAR)


def draw_logo():
    # Rounded-square background
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, S - 1, S - 1), radius=230, fill=255)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(gradient(S), (0, 0), mask)

    d = ImageDraw.Draw(img)

    # Monitor
    left, top, right, bottom = 190, 230, 834, 690
    d.rounded_rectangle((left, top, right, bottom), radius=60, fill=SCREEN, outline=WHITE, width=56)
    # Stand: neck + base
    d.rectangle((462, bottom, 562, 790), fill=WHITE)
    d.rounded_rectangle((330, 770, 694, 830), radius=30, fill=WHITE)

    # Mouse pointer on the screen
    ox, oy = 430, 320
    pointer = [
        (ox, oy),
        (ox, oy + 300),
        (ox + 78, oy + 228),
        (ox + 130, oy + 330),
        (ox + 185, oy + 304),
        (ox + 134, oy + 205),
        (ox + 236, oy + 198),
    ]
    d.polygon(pointer, fill=WHITE)

    return img


def main():
    logo = draw_logo()
    logo.resize((256, 256), Image.LANCZOS).save("icon.png")
    logo.save(
        "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )


if __name__ == "__main__":
    main()
