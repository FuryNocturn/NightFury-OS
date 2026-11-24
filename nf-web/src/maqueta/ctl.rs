// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! Tamaño de los controles de formulario (como en un navegador de escritorio: campo de 20 letras, área de
//! 20×2, casillas de 13 px). El ancho y el alto de la hoja de estilos mandan y cuentan como la caja entera.
use super::*;
use crate::forms::{alto_linea, Tipo};

impl<'a> Maq<'a> {
    /// (ancho, alto) en px del control `k` (nodo `n`) con `avail` px de ancho disponible.
    pub(super) fn tam_control(&mut self, n: usize, k: u32, avail: i32) -> (i32, i32) {
        let forms = self.forms;
        let c = &forms.ctl[k as usize];
        let est = self.est;
        let e = est.de(n);
        let lh = alto_linea(e.px);
        let cero = self.medir(e, "0").max(1);
        let boton = |s: &mut Self, t: &str| (s.medir(e, t).saturating_add(16), lh + 6);
        let (mut w, mut h) = match c.tipo {
            Tipo::Oculto => (0, 0),
            Tipo::Casilla | Tipo::Radio => (13, 13),
            Tipo::Texto | Tipo::Password => ((c.tam as i32).saturating_mul(cero).saturating_add(8), lh + 6),
            Tipo::Area => ((c.tam as i32).saturating_mul(cero).saturating_add(8), (c.filas as i32).saturating_mul(lh).saturating_add(6)),
            Tipo::Select => {
                let mut mx = 30;
                for o in &c.opciones {
                    mx = mx.max(self.medir(e, &o.etiqueta));
                }
                (mx.saturating_add(26), lh + 6)
            }
            Tipo::Enviar | Tipo::Restablecer | Tipo::Boton | Tipo::Imagen => boton(self, &c.etiqueta),
        };
        if c.tipo != Tipo::Oculto {
            if let Some(x) = largo(e.ancho, avail) {
                w = x.max(1);
            }
            if let Some(x) = largo(e.alto, avail) {
                h = x.max(1);
            }
        }
        (w.clamp(0, MAX_ANCHO), h.clamp(0, MAX_ALTO))
    }
}
