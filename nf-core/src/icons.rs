// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Iconos propios dibujados por código. CONTRATO: `Icon` y la firma de `draw` no cambian.
//! Rejilla lógica de 16 unidades; `k = size / 16`. Los rectángulos se ajustan a píxeles enteros.
use crate::geom::Rect;
use crate::math::{cos, iround, sin, PI};
use crate::surface::{rgb, Surface};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Notepad,
    Calc,
    Terminal,
    Settings,
    Power,
    /// Logo del sistema (luna creciente).
    Logo,
    Info,
    Palette,
    /// Carpeta.
    Folder,
    /// Archivo de texto.
    FileText,
    /// Archivo de tipo desconocido.
    FileBlank,
    /// Papelera.
    Trash,
    /// Unidad de disco.
    Drive,
    Copiar,
    Pegar,
    Cortar,
    MoverA,
    CopiarA,
    Eliminar,
    CambiarNombre,
    CarpetaNueva,
    DocumentoNuevo,
    Propiedades,
    Abrir,
    SeleccionarTodo,
    SeleccionarNada,
    InvertirSeleccion,
    VistaIconosGrandes,
    VistaIconosMedianos,
    VistaIconosPequenos,
    VistaLista,
    VistaDetalles,
    Ordenar,
    VentanaNueva,
    Cerrar,
    /// Carpetas estándar del usuario.
    Escritorio,
    Documentos,
    Descargas,
    Imagenes,
    Musica,
    Videos,
    /// Cuentas de usuario (busto).
    Cuentas,
    /// Disco con candado (cifrado).
    DiscoSeguro,
    /// Administrador de tareas: panel con barras de uso.
    Tareas,
    /// Pendrive (unidad extraíble).
    Usb,
    /// Pendrive con candado (unidad extraíble cifrada).
    UsbSeguro,
    /// Este equipo: pantalla sobre una base.
    Equipo,
}

/// Rectángulo en unidades de rejilla (x0,y0)-(x1,y1), con bordes redondeados a píxel y mínimo 1 px.
fn cell(s: &mut Surface, o: (i32, i32), k: f32, r: (f32, f32, f32, f32), c: u32) {
    let (x0, y0) = (iround(r.0 * k), iround(r.1 * k));
    let (x1, y1) = (iround(r.2 * k).max(x0 + 1), iround(r.3 * k).max(y0 + 1));
    s.fill_rect(Rect::new(o.0 + x0, o.1 + y0, x1 - x0, y1 - y0), c);
}

const AZUL: u32 = 0x2F7BE0;
const CLARO: u32 = 0xBFD6F2;
const GRIS: u32 = 0x8A9099;
const OSCURO: u32 = 0x44484D;
const ROJO: u32 = 0xD94141;
const VERDE: u32 = 0x3CB371;

/// Hoja rectangular: borde de 1 unidad de rejilla y relleno.
fn sheet(s: &mut Surface, o: (i32, i32), k: f32, r: (f32, f32, f32, f32), borde: u32, fill: u32) {
    cell(s, o, k, r, borde);
    cell(s, o, k, (r.0 + 1.0, r.1 + 1.0, r.2 - 1.0, r.3 - 1.0), fill);
}

/// Disco de color con un "+" blanco (nuevo / copiar), centrado en `c`.
fn insignia(s: &mut Surface, c: (f32, f32), k: f32, col: u32) {
    s.fill_circle(c.0, c.1, 3.4 * k, col, 255);
    let t = (1.3 * k).max(1.3);
    s.stroke(&[(c.0 - 1.8 * k, c.1), (c.0 + 1.8 * k, c.1)], t, 0xFFFFFF);
    s.stroke(&[(c.0, c.1 - 1.8 * k), (c.0, c.1 + 1.8 * k)], t, 0xFFFFFF);
}

/// Dibuja `icon` en el cuadrado de lado `size` con esquina en (x, y).
/// `fg` es el color de los iconos de un solo tono (Settings, Power, Logo, Info).
pub fn draw(s: &mut Surface, icon: Icon, x: i32, y: i32, size: i32, fg: u32) {
    if size <= 0 {
        return;
    }
    // Todo el dibujo queda recortado al cuadrado del icono.
    let old = s.clip();
    s.set_clip(old.intersect(Rect::new(x, y, size, size)));
    let k = size as f32 / 16.0;
    let o = (x, y);
    // Punto de la rejilla en coordenadas de superficie.
    let p = |u: f32, v: f32| (x as f32 + u * k, y as f32 + v * k);
    // Grosor de trazo en píxeles, nunca menor de 1,3 para que se vea a 16 px.
    let tw = |u: f32| (u * k).max(1.3);
    match icon {
        Icon::Notepad => {
            cell(s, o, k, (3.0, 1.0, 13.0, 15.0), rgb(0x9A, 0xA0, 0xA6));
            cell(s, o, k, (4.0, 2.0, 12.0, 14.0), 0xFFFFFF);
            cell(s, o, k, (4.0, 2.0, 12.0, 4.0), rgb(0x2F, 0x7B, 0xE0));
            for (i, yy) in [6.0f32, 8.0, 10.0, 12.0].iter().enumerate() {
                let x1 = if i == 3 { 8.0 } else { 11.0 };
                cell(s, o, k, (5.0, *yy, x1, yy + 1.0), rgb(0xA8, 0xAD, 0xB3));
            }
        }
        Icon::Calc => {
            cell(s, o, k, (2.0, 1.0, 14.0, 16.0), rgb(0x3C, 0x40, 0x43));
            cell(s, o, k, (3.0, 3.0, 13.0, 6.0), rgb(0xD6, 0xE6, 0xDA));
            for r in 0..3 {
                for c in 0..3 {
                    let (kx, ky) = (3.0 + c as f32 * 3.5, 7.0 + r as f32 * 3.0);
                    let col = if r == 2 && c == 2 { rgb(0xF2, 0x8B, 0x30) } else { rgb(0xC9, 0xCD, 0xD2) };
                    cell(s, o, k, (kx, ky, kx + 2.5, ky + 2.0), col);
                }
            }
        }
        Icon::Terminal => {
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), rgb(0xB4, 0xB8, 0xBD));
            cell(s, o, k, (2.0, 3.0, 14.0, 13.0), rgb(0x0C, 0x0C, 0x0C));
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), rgb(0x5C, 0x60, 0x66));
            let t = tw(1.1);
            let (a, b, c) = (p(3.8, 6.8), p(6.4, 8.8), p(3.8, 10.8));
            s.stroke(&[a, b, c], t, 0xFFFFFF);
            s.stroke(&[p(8.0, 11.0), p(11.5, 11.0)], t, 0xFFFFFF);
        }
        Icon::Settings => {
            let (cx, cy) = p(8.0, 8.0);
            s.arc(cx, cy, 3.7 * k, 0.0, 360.0, tw(2.6), fg);
            for i in 0..8 {
                let a = (i as f32 * 45.0 + 22.5) * PI / 180.0;
                let (c, sn) = (cos(a), sin(a));
                let t = tw(2.4);
                s.stroke(&[(cx + c * 4.4 * k, cy + sn * 4.4 * k), (cx + c * 5.9 * k, cy + sn * 5.9 * k)], t, fg);
            }
        }
        Icon::Power => {
            let (cx, cy) = p(8.0, 8.8);
            s.arc(cx, cy, 4.8 * k, 125.0, 415.0, tw(1.7), fg);
            s.stroke(&[p(8.0, 2.4), p(8.0, 8.0)], tw(1.7), fg);
        }
        Icon::Logo => {
            let (cx, cy) = p(8.0, 8.0);
            let (ox, oy) = p(11.2, 4.8);
            s.crescent(cx, cy, 7.2 * k, ox, oy, 5.9 * k, fg);
        }
        Icon::Info => {
            let (cx, cy) = p(8.0, 8.0);
            s.arc(cx, cy, 6.3 * k, 0.0, 360.0, tw(1.5), fg);
            let (dx, dy) = p(8.0, 4.9);
            s.fill_circle(dx, dy, (0.95 * k).max(0.9), fg, 255);
            s.stroke(&[p(8.0, 7.3), p(8.0, 11.6)], tw(1.8), fg);
        }
        Icon::Palette => {
            // Paleta de pintor: disco beige con agujero del pulgar y cuatro gotas de color.
            let (cx, cy) = p(8.0, 8.0);
            s.fill_circle(cx, cy, 7.0 * k, rgb(0xE6, 0xCF, 0xA3), 255);
            let (hx, hy) = p(11.0, 11.6);
            s.fill_circle(hx, hy, 1.5 * k, rgb(0x7A, 0x66, 0x44), 255);
            let dots = [(4.6, 7.2, 0xE0_4A_4A), (6.6, 3.9, 0xF2_B0_1E), (10.4, 4.3, 0x2F_7B_E0), (4.6, 10.8, 0x3C_B3_71)];
            for (u, v, c) in dots {
                let (dx, dy) = p(u, v);
                s.fill_circle(dx, dy, (1.45 * k).max(1.2), c, 255);
            }
        }
        Icon::Folder => {
            // Pestaña y cuerpo trasero ámbar, frente más oscuro.
            let (d, l) = (rgb(0xE0, 0x9F, 0x1E), rgb(0xF6, 0xC8, 0x4C));
            cell(s, o, k, (1.0, 2.5, 6.5, 5.0), d);
            cell(s, o, k, (1.0, 4.0, 15.0, 13.0), l);
            cell(s, o, k, (1.0, 6.5, 15.0, 13.5), d);
        }
        Icon::FileText | Icon::FileBlank => {
            let (borde, plie) = (rgb(0x8A, 0x90, 0x99), rgb(0xD3, 0xD7, 0xDC));
            // Hoja con la esquina superior derecha doblada (pliegue triangular escalonado).
            cell(s, o, k, (3.0, 1.0, 9.0, 15.0), borde);
            cell(s, o, k, (9.0, 5.0, 13.0, 15.0), borde);
            cell(s, o, k, (4.0, 2.0, 9.0, 14.0), 0xFFFFFF);
            cell(s, o, k, (9.0, 6.0, 12.0, 14.0), 0xFFFFFF);
            cell(s, o, k, (9.0, 1.0, 10.0, 6.0), borde);
            cell(s, o, k, (9.0, 5.0, 13.0, 6.0), borde);
            cell(s, o, k, (10.0, 2.0, 12.0, 5.0), plie);
            if icon == Icon::FileText {
                for (i, yy) in [7.0f32, 9.0, 11.0].iter().enumerate() {
                    let x1 = if i == 2 { 8.0 } else { 11.0 };
                    cell(s, o, k, (5.0, *yy, x1, yy + 1.0), rgb(0xA8, 0xAD, 0xB3));
                }
            }
        }
        Icon::Trash => {
            // Tapa y asa.
            cell(s, o, k, (2.5, 3.5, 13.5, 4.8), fg);
            cell(s, o, k, (6.0, 1.5, 10.0, 2.5), fg);
            cell(s, o, k, (6.0, 1.5, 7.0, 3.5), fg);
            cell(s, o, k, (9.0, 1.5, 10.0, 3.5), fg);
            // Cubo recto (trazos a píxel, nítido a 16 px) con dos rayas.
            cell(s, o, k, (3.8, 5.0, 5.0, 15.0), fg);
            cell(s, o, k, (11.0, 5.0, 12.2, 15.0), fg);
            cell(s, o, k, (3.8, 14.0, 12.2, 15.0), fg);
            cell(s, o, k, (6.6, 7.0, 7.6, 12.5), fg);
            cell(s, o, k, (8.4, 7.0, 9.4, 12.5), fg);
        }
        Icon::Drive => {
            let (c, d) = (rgb(0xB9, 0xBE, 0xC5), rgb(0x7E, 0x84, 0x8C));
            cell(s, o, k, (1.0, 5.0, 15.0, 12.0), d);
            cell(s, o, k, (1.0, 5.0, 15.0, 10.5), c);
            cell(s, o, k, (1.0, 5.0, 15.0, 6.0), rgb(0xD8, 0xDB, 0xE0));
            cell(s, o, k, (3.0, 7.5, 9.0, 8.8), rgb(0x3A, 0x3D, 0x42));
            let (lx, ly) = p(12.4, 8.2);
            s.fill_circle(lx, ly, (0.9 * k).max(0.9), rgb(0x2F, 0xC2, 0x5B), 255);
            cell(s, o, k, (3.0, 11.0, 6.0, 11.5), rgb(0x5E, 0x63, 0x6A));
        }
        Icon::Copiar => {
            sheet(s, o, k, (2.0, 1.0, 10.0, 11.0), GRIS, 0xFFFFFF);
            sheet(s, o, k, (6.0, 5.0, 14.0, 15.0), AZUL, 0xFFFFFF);
            cell(s, o, k, (8.0, 8.0, 12.0, 9.0), CLARO);
            cell(s, o, k, (8.0, 10.5, 12.0, 11.5), CLARO);
        }
        Icon::Pegar => {
            cell(s, o, k, (2.0, 3.0, 14.0, 15.0), rgb(0xC0, 0x8A, 0x4A));
            sheet(s, o, k, (4.0, 5.0, 12.0, 14.0), GRIS, 0xFFFFFF);
            cell(s, o, k, (5.0, 1.5, 11.0, 4.5), OSCURO);
            cell(s, o, k, (6.0, 8.0, 10.0, 9.0), CLARO);
            cell(s, o, k, (6.0, 10.5, 10.0, 11.5), CLARO);
        }
        Icon::Cortar => {
            let t = tw(1.3);
            s.stroke(&[p(4.5, 1.5), p(10.5, 10.5)], t, OSCURO);
            s.stroke(&[p(11.5, 1.5), p(5.5, 10.5)], t, OSCURO);
            for u in [4.5f32, 11.5] {
                let (cx, cy) = p(u, 12.5);
                s.arc(cx, cy, 2.0 * k, 0.0, 360.0, tw(1.3), ROJO);
            }
        }
        Icon::MoverA | Icon::CopiarA => {
            let (d, l) = (rgb(0xE0, 0x9F, 0x1E), rgb(0xF6, 0xC8, 0x4C));
            cell(s, o, k, (1.0, 2.0, 6.0, 4.0), d);
            cell(s, o, k, (1.0, 3.0, 12.0, 11.0), l);
            cell(s, o, k, (1.0, 5.0, 12.0, 11.0), d);
            if icon == Icon::MoverA {
                s.stroke(&[p(6.0, 13.5), p(14.0, 13.5)], tw(1.5), AZUL);
                s.stroke(&[p(11.5, 11.0), p(14.5, 13.5), p(11.5, 16.0)], tw(1.5), AZUL);
            } else {
                insignia(s, p(12.0, 11.5), k, VERDE);
            }
        }
        Icon::Eliminar => {
            let t = tw(2.2);
            s.stroke(&[p(3.5, 3.5), p(12.5, 12.5)], t, ROJO);
            s.stroke(&[p(12.5, 3.5), p(3.5, 12.5)], t, ROJO);
        }
        Icon::CambiarNombre => {
            cell(s, o, k, (1.0, 4.0, 15.0, 12.0), GRIS);
            cell(s, o, k, (2.0, 5.0, 14.0, 11.0), 0xFFFFFF);
            cell(s, o, k, (3.0, 7.5, 7.0, 8.5), CLARO);
            s.stroke(&[p(9.5, 2.0), p(9.5, 14.0)], tw(1.2), AZUL);
            s.stroke(&[p(8.0, 2.0), p(11.0, 2.0)], tw(1.2), AZUL);
            s.stroke(&[p(8.0, 14.0), p(11.0, 14.0)], tw(1.2), AZUL);
        }
        Icon::CarpetaNueva => {
            draw(s, Icon::Folder, x, y + iround(1.5 * k), size, fg);
            insignia(s, p(12.0, 4.0), k, VERDE);
        }
        Icon::DocumentoNuevo => {
            draw(s, Icon::FileBlank, x, y, size, fg);
            insignia(s, p(12.0, 12.0), k, VERDE);
        }
        Icon::Propiedades => {
            sheet(s, o, k, (3.0, 1.0, 13.0, 15.0), GRIS, 0xFFFFFF);
            cell(s, o, k, (5.0, 4.0, 11.0, 5.0), CLARO);
            cell(s, o, k, (5.0, 6.5, 9.0, 7.5), CLARO);
            s.stroke(&[p(5.5, 11.0), p(7.5, 13.0), p(11.5, 8.5)], tw(1.5), VERDE);
        }
        Icon::Abrir => {
            sheet(s, o, k, (2.0, 3.0, 11.0, 15.0), GRIS, 0xFFFFFF);
            cell(s, o, k, (4.0, 6.0, 8.0, 7.0), CLARO);
            s.stroke(&[p(7.0, 10.0), p(14.0, 3.0)], tw(1.6), AZUL);
            s.stroke(&[p(9.5, 2.5), p(14.5, 2.5), p(14.5, 7.5)], tw(1.6), AZUL);
        }
        Icon::SeleccionarTodo => {
            sheet(s, o, k, (2.0, 2.0, 14.0, 14.0), AZUL, CLARO);
            s.stroke(&[p(4.8, 8.2), p(7.2, 10.8), p(11.5, 5.2)], tw(1.6), AZUL);
        }
        Icon::SeleccionarNada => {
            // Marco discontinuo: borde gris con huecos en el centro de cada lado.
            sheet(s, o, k, (2.0, 2.0, 14.0, 14.0), GRIS, 0xFFFFFF);
            cell(s, o, k, (6.0, 2.0, 10.0, 3.0), 0xFFFFFF);
            cell(s, o, k, (6.0, 13.0, 10.0, 14.0), 0xFFFFFF);
            cell(s, o, k, (2.0, 6.0, 3.0, 10.0), 0xFFFFFF);
            cell(s, o, k, (13.0, 6.0, 14.0, 10.0), 0xFFFFFF);
        }
        Icon::InvertirSeleccion => {
            sheet(s, o, k, (2.0, 2.0, 14.0, 14.0), AZUL, 0xFFFFFF);
            cell(s, o, k, (3.0, 3.0, 8.0, 13.0), AZUL);
        }
        Icon::VistaIconosGrandes | Icon::VistaIconosMedianos | Icon::VistaIconosPequenos => {
            // n x n cuadrados con hueco dentro de 1..15.
            let n = match icon {
                Icon::VistaIconosGrandes => 2,
                Icon::VistaIconosMedianos => 3,
                _ => 4,
            };
            let gap = 1.6 - 0.1 * n as f32;
            let lado = (14.0 - gap * (n - 1) as f32) / n as f32;
            for j in 0..n {
                for i in 0..n {
                    let (u, v) = (1.0 + i as f32 * (lado + gap), 1.0 + j as f32 * (lado + gap));
                    cell(s, o, k, (u, v, u + lado, v + lado), AZUL);
                }
            }
        }
        Icon::VistaLista => {
            for v in [2.5f32, 6.5, 10.5] {
                cell(s, o, k, (1.5, v, 4.5, v + 3.0), AZUL);
                cell(s, o, k, (6.0, v + 1.0, 14.5, v + 2.0), GRIS);
            }
        }
        Icon::VistaDetalles => {
            cell(s, o, k, (1.0, 2.0, 15.0, 5.0), AZUL);
            for v in [6.5f32, 9.0, 11.5, 14.0] {
                cell(s, o, k, (1.0, v, 5.0, v + 1.0), GRIS);
                cell(s, o, k, (6.5, v, 15.0, v + 1.0), CLARO);
            }
        }
        Icon::Ordenar => {
            cell(s, o, k, (1.5, 3.0, 5.0, 4.5), GRIS);
            cell(s, o, k, (1.5, 7.0, 8.0, 8.5), GRIS);
            cell(s, o, k, (1.5, 11.0, 10.0, 12.5), GRIS);
            s.stroke(&[p(13.0, 2.5), p(13.0, 13.0)], tw(1.4), AZUL);
            s.stroke(&[p(10.8, 10.8), p(13.0, 13.2), p(15.2, 10.8)], tw(1.4), AZUL);
        }
        Icon::VentanaNueva => {
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), GRIS);
            cell(s, o, k, (2.0, 5.0, 14.0, 13.0), 0xFFFFFF);
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), AZUL);
            insignia(s, p(11.5, 11.0), k, VERDE);
        }
        Icon::Escritorio => {
            // Monitor con el fondo azul y una barra de tareas.
            cell(s, o, k, (1.0, 2.0, 15.0, 11.0), OSCURO);
            cell(s, o, k, (2.0, 3.0, 14.0, 10.0), AZUL);
            cell(s, o, k, (2.0, 8.5, 14.0, 10.0), CLARO);
            cell(s, o, k, (3.5, 4.5, 6.0, 7.0), 0xFFFFFF);
            cell(s, o, k, (7.0, 11.0, 9.0, 13.0), OSCURO);
            cell(s, o, k, (4.5, 13.0, 11.5, 14.5), OSCURO);
        }
        Icon::Documentos => {
            // Hoja con cabecera azul, texto y un doblez; otra hoja asoma detrás.
            sheet(s, o, k, (2.0, 2.5, 11.0, 15.0), GRIS, 0xFFFFFF);
            sheet(s, o, k, (5.0, 1.0, 14.0, 13.5), AZUL, 0xFFFFFF);
            cell(s, o, k, (6.0, 2.0, 13.0, 4.5), AZUL);
            for (i, yy) in [6.0f32, 8.0, 10.0].iter().enumerate() {
                let x1 = if i == 2 { 10.0 } else { 12.0 };
                cell(s, o, k, (7.0, *yy, x1, yy + 1.0), CLARO);
            }
        }
        Icon::Descargas => {
            // Disco verde con una flecha blanca hacia abajo sobre una bandeja.
            let (cx, cy) = p(8.0, 8.0);
            s.fill_circle(cx, cy, 7.2 * k, VERDE, 255);
            let t = tw(1.8);
            s.stroke(&[p(8.0, 3.8), p(8.0, 9.6)], t, 0xFFFFFF);
            s.stroke(&[p(5.0, 7.0), p(8.0, 10.0), p(11.0, 7.0)], t, 0xFFFFFF);
            cell(s, o, k, (4.5, 11.3, 11.5, 12.6), 0xFFFFFF);
        }
        Icon::Imagenes => {
            // Marco con cielo, sol y montañas.
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), GRIS);
            cell(s, o, k, (2.0, 3.0, 14.0, 13.0), CLARO);
            let (sx, sy) = p(11.3, 5.7);
            s.fill_circle(sx, sy, (1.4 * k).max(1.2), rgb(0xF6, 0xC8, 0x4C), 255);
            cell(s, o, k, (2.0, 11.0, 14.0, 13.0), VERDE);
            s.stroke(&[p(2.5, 11.0), p(6.0, 7.0), p(9.5, 11.0)], tw(2.6), rgb(0x2E, 0x8B, 0x57));
            s.stroke(&[p(8.0, 11.0), p(11.0, 8.6), p(13.8, 11.0)], tw(2.2), rgb(0x3C, 0x9F, 0x66));
            cell(s, o, k, (2.0, 11.0, 14.0, 13.0), VERDE);
        }
        Icon::Musica => {
            // Corchea doble: dos cabezas, dos plicas y la barra.
            let col = rgb(0x8E, 0x4B, 0xC8);
            let (ax, ay) = p(4.8, 12.2);
            let (bx, by) = p(11.4, 10.8);
            s.fill_circle(ax, ay, (2.0 * k).max(1.7), col, 255);
            s.fill_circle(bx, by, (2.0 * k).max(1.7), col, 255);
            let t = tw(1.3);
            s.stroke(&[p(6.5, 11.8), p(6.5, 2.8)], t, col);
            s.stroke(&[p(13.1, 10.4), p(13.1, 1.8)], t, col);
            s.stroke(&[p(6.5, 2.6), p(13.1, 1.6)], tw(2.0), col);
        }
        Icon::Videos => {
            // Fotograma oscuro con perforaciones y un triángulo de reproducir.
            cell(s, o, k, (1.0, 3.0, 15.0, 13.0), OSCURO);
            for i in 0..4 {
                let y0 = 4.0 + i as f32 * 2.4;
                cell(s, o, k, (1.8, y0, 2.9, y0 + 1.2), 0xFFFFFF);
                cell(s, o, k, (13.1, y0, 14.2, y0 + 1.2), 0xFFFFFF);
            }
            for i in 0..5 {
                let x0 = 5.4 + i as f32 * 1.0;
                let h = 3.2 - i as f32 * 0.64;
                cell(s, o, k, (x0, 8.0 - h, x0 + 1.0, 8.0 + h), rgb(0xF2, 0x8B, 0x30));
            }
        }
        Icon::Cuentas => {
            // Busto: cabeza y hombros.
            let (hx, hy) = p(8.0, 5.2);
            s.fill_circle(hx, hy, 3.0 * k, fg, 255);
            let (bx, by) = p(8.0, 15.5);
            s.fill_circle(bx, by, 6.0 * k, fg, 255);
        }
        Icon::DiscoSeguro => {
            // Unidad de disco con un candado dorado delante.
            draw(s, Icon::Drive, x, y - iround(1.5 * k), size, fg);
            cell(s, o, k, (9.0, 10.0, 15.0, 15.5), rgb(0xE0, 0x9F, 0x1E));
            cell(s, o, k, (10.0, 8.0, 11.0, 10.0), OSCURO);
            cell(s, o, k, (13.0, 8.0, 14.0, 10.0), OSCURO);
            cell(s, o, k, (10.0, 7.2, 14.0, 8.2), OSCURO);
            cell(s, o, k, (11.6, 11.8, 12.4, 13.6), OSCURO);
        }
        Icon::Tareas => {
            // Panel oscuro con una cabecera azul y barras de uso a distintas alturas.
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), OSCURO);
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), AZUL);
            cell(s, o, k, (2.0, 5.0, 14.0, 13.0), rgb(0x23, 0x26, 0x29));
            cell(s, o, k, (3.5, 10.0, 5.5, 12.0), VERDE);
            cell(s, o, k, (6.5, 8.0, 8.5, 12.0), VERDE);
            cell(s, o, k, (9.5, 6.5, 11.5, 12.0), rgb(0xF2, 0xB0, 0x1E));
            cell(s, o, k, (12.0, 9.0, 13.0, 12.0), ROJO);
            cell(s, o, k, (3.0, 12.4, 13.0, 12.8), GRIS);
        }
        Icon::Usb | Icon::UsbSeguro => {
            // Pendrive: conector metálico con dos agujeros y cuerpo azul con una ranura de anilla.
            cell(s, o, k, (6.0, 1.0, 10.0, 6.0), rgb(0xB9, 0xBE, 0xC5));
            cell(s, o, k, (6.8, 2.0, 7.6, 3.4), OSCURO);
            cell(s, o, k, (8.4, 2.0, 9.2, 3.4), OSCURO);
            cell(s, o, k, (5.0, 6.0, 11.0, 15.0), AZUL);
            cell(s, o, k, (5.0, 6.0, 11.0, 7.0), rgb(0x5B, 0x98, 0xEA));
            cell(s, o, k, (7.4, 12.6, 8.6, 13.6), CLARO);
            if icon == Icon::UsbSeguro {
                cell(s, o, k, (8.0, 9.0, 15.0, 15.5), rgb(0xE0, 0x9F, 0x1E));
                cell(s, o, k, (9.0, 7.0, 10.0, 9.0), OSCURO);
                cell(s, o, k, (13.0, 7.0, 14.0, 9.0), OSCURO);
                cell(s, o, k, (9.0, 6.2, 14.0, 7.2), OSCURO);
                cell(s, o, k, (11.0, 11.4, 12.0, 13.4), OSCURO);
            }
        }
        Icon::Equipo => {
            // Pantalla con marco oscuro sobre un pie y una base.
            cell(s, o, k, (1.0, 2.0, 15.0, 11.0), OSCURO);
            cell(s, o, k, (2.0, 3.0, 14.0, 10.0), rgb(0x8F, 0xBE, 0xF0));
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), rgb(0xB5, 0xD3, 0xF6));
            cell(s, o, k, (7.0, 11.0, 9.0, 13.0), GRIS);
            cell(s, o, k, (4.0, 13.0, 12.0, 14.5), GRIS);
        }
        Icon::Cerrar => {
            cell(s, o, k, (1.0, 2.0, 15.0, 14.0), GRIS);
            cell(s, o, k, (2.0, 5.0, 14.0, 13.0), 0xFFFFFF);
            cell(s, o, k, (2.0, 3.0, 14.0, 5.0), AZUL);
            let t = tw(1.7);
            s.stroke(&[p(5.5, 6.5), p(10.5, 11.5)], t, ROJO);
            s.stroke(&[p(10.5, 6.5), p(5.5, 11.5)], t, ROJO);
        }
    }
    s.set_clip(old);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 47] = [
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
        Icon::Copiar,
        Icon::Pegar,
        Icon::Cortar,
        Icon::MoverA,
        Icon::CopiarA,
        Icon::Eliminar,
        Icon::CambiarNombre,
        Icon::CarpetaNueva,
        Icon::DocumentoNuevo,
        Icon::Propiedades,
        Icon::Abrir,
        Icon::SeleccionarTodo,
        Icon::SeleccionarNada,
        Icon::InvertirSeleccion,
        Icon::VistaIconosGrandes,
        Icon::VistaIconosMedianos,
        Icon::VistaIconosPequenos,
        Icon::VistaLista,
        Icon::VistaDetalles,
        Icon::Ordenar,
        Icon::VentanaNueva,
        Icon::Cerrar,
        Icon::Escritorio,
        Icon::Documentos,
        Icon::Descargas,
        Icon::Imagenes,
        Icon::Musica,
        Icon::Videos,
        Icon::Cuentas,
        Icon::DiscoSeguro,
        Icon::Tareas,
        Icon::Usb,
        Icon::UsbSeguro,
        Icon::Equipo,
    ];
    const SENT: u32 = 0x123456;

    #[test]
    fn draws_inside_its_square_only() {
        for icon in ALL {
            for size in [16, 24, 32, 40] {
                let mut s = Surface::new(size + 20, size + 20, SENT);
                draw(&mut s, icon, 10, 10, size, 0xFFFFFF);
                let (mut dentro, mut fuera) = (false, false);
                for j in 0..s.h {
                    for i in 0..s.w {
                        let changed = s.px[(j * s.w + i) as usize] != SENT;
                        if (10..10 + size).contains(&i) && (10..10 + size).contains(&j) {
                            dentro |= changed;
                        } else {
                            fuera |= changed;
                        }
                    }
                }
                assert!(dentro, "{icon:?} {size}: no dibuja nada");
                assert!(!fuera, "{icon:?} {size}: escribe fuera");
            }
        }
    }

    #[test]
    fn edges_do_not_panic() {
        let mut s = Surface::new(10, 10, 0);
        for icon in ALL {
            draw(&mut s, icon, -8, -8, 16, 0xFFFFFF);
            draw(&mut s, icon, 5, 5, 40, 0xFFFFFF);
            draw(&mut s, icon, 0, 0, 0, 0xFFFFFF);
        }
    }
}
