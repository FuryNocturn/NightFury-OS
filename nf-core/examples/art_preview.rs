// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Vista previa del arte: fondos e iconos en una sola imagen BMP.
//! Uso: cargo run --release -p nf-core --example art_preview -- salida.bmp
use nf_core::geom::Rect;
use nf_core::icons::{draw, Icon};
use nf_core::surface::Surface;
use nf_core::theme::{render_wallpaper, ACCENTS};
use std::time::Instant;

const ICONS: [Icon; 13] = [
    Icon::Notepad,
    Icon::Calc,
    Icon::Terminal,
    Icon::Settings,
    Icon::Power,
    Icon::Logo,
    Icon::Info,
    Icon::Palette,
    Icon::Folder,
    Icon::FileText,
    Icon::FileBlank,
    Icon::Trash,
    Icon::Drive,
];

fn main() {
    let out = std::env::args().nth(1).expect("falta la ruta de salida .bmp");
    let accent = ACCENTS[0];
    let mut img = Surface::new(2800, 1260, 0x404040);

    // Fondos a 640x400 y miniaturas de 128x72.
    for id in 0..4u8 {
        let t = Instant::now();
        let w = render_wallpaper(id, 640, 400, accent);
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        img.blit(&w, 10 + (id as i32 % 2) * 650, 10 + (id as i32 / 2) * 410);
        let m = render_wallpaper(id, 128, 72, accent);
        img.blit(&m, 10 + id as i32 * 138, 840);
        eprintln!("fondo {id}: 640x400 en {ms:.1} ms");
    }
    for id in 0..3u8 {
        let t = Instant::now();
        let _ = render_wallpaper(id, 1920, 1080, accent);
        eprintln!("fondo {id}: 1920x1080 en {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
    }

    // Iconos sobre tres fondos: (color de fondo, fg).
    let fondos = [(0xFFFFFF, 0x202020), (0x1F1F1F, 0xFFFFFF), (accent, 0xFFFFFF)];
    for (b, &(bg, fg)) in fondos.iter().enumerate() {
        let (x0, y0) = (10 + b as i32 * 930, 930);
        img.fill_rect(Rect::new(x0, y0, 920, 320), bg);
        for (i, &ic) in ICONS.iter().enumerate() {
            let x = x0 + 8 + i as i32 * 70;
            let mut y = y0 + 8;
            for size in [16, 24, 32, 40] {
                draw(&mut img, ic, x, y, size, fg);
                y += size + 6;
            }
            // 16 px ampliado x4.
            let mut small = Surface::new(16, 16, bg);
            draw(&mut small, ic, 0, 0, 16, fg);
            for j in 0..64 {
                for i2 in 0..64 {
                    let p = small.px[((j / 4) * 16 + i2 / 4) as usize];
                    img.px[((y + 4 + j) * img.w + x + i2) as usize] = p;
                }
            }
        }
    }
    std::fs::write(&out, nf_core::bmp::encode(&img)).unwrap();
    eprintln!("escrito {out}");
}
