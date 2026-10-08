import { Ac as defineStd, gs as trapezoid } from "./std-ClftfWpF.js";
function halfUvFromProp(value, canvasHeight, fallback) {
	const safeFallback = Number.isFinite(fallback) ? fallback : .5;
	if (value != null && typeof value === "object" && typeof value.value === "number") {
		const dv = value;
		if (!Number.isFinite(dv.value)) return safeFallback;
		if (dv.unit !== "px") return dv.value;
		return Number.isFinite(canvasHeight) && canvasHeight !== 0 ? dv.value / (2 * canvasHeight) : safeFallback;
	}
	return typeof value === "number" && Number.isFinite(value) ? value : safeFallback;
}
const componentDefinition = defineStd({
	role: "shape",
	name: "Trapezoid",
	description: "Trapezoid with adjustable top and bottom widths and height",
	shape: {
		distance: ({ x, y }, u) => trapezoid(x, y, u("topWidth"), u("bottomWidth"), u("height")),
		colorDescription: "Fill color of the trapezoid",
		centerDescription: "Center position of the trapezoid",
		bounds: { custom: {
			propBindings: {
				x: {
					prop: "center",
					as: "position-x"
				},
				y: {
					prop: "center",
					as: "position-y"
				},
				rotation: {
					prop: "rotation",
					as: "degrees"
				}
			},
			computeBounds(props, cw, ch) {
				const bottom = halfUvFromProp(props.bottomWidth, ch, .35);
				const top = typeof props.topWidth === "number" ? props.topWidth : .2;
				const halfH = halfUvFromProp(props.height, ch, .25);
				const cx = typeof props.center?.x === "number" ? props.center.x : .5;
				const cy = typeof props.center?.y === "number" ? props.center.y : .5;
				return {
					centerXPx: cx * cw,
					centerYPx: cy * ch,
					widthPx: Math.max(top, bottom) * 2 * ch,
					heightPx: halfH * 2 * ch,
					rotationDeg: props.rotation ?? 0
				};
			},
			writeBounds(bounds, currentProps, cw, ch) {
				const keepUnit = (cur, newHalfUV) => cur != null && typeof cur === "object" && cur.unit === "px" ? {
					value: Math.max(0, Math.round(newHalfUV * 2 * ch)),
					unit: "px"
				} : Math.max(.001, newHalfUV);
				const bottom = halfUvFromProp(currentProps.bottomWidth, ch, .35);
				const top = typeof currentProps.topWidth === "number" ? currentProps.topWidth : .2;
				const oldMax = Math.max(top, bottom);
				const newMax = ch > 0 ? bounds.widthPx / (2 * ch) : oldMax;
				const factor = oldMax > 0 ? newMax / oldMax : 1;
				const prevCenter = currentProps.center ?? {
					x: .5,
					y: .5
				};
				return {
					center: {
						...prevCenter,
						x: cw > 0 ? bounds.centerXPx / cw : prevCenter.x ?? .5,
						y: ch > 0 ? bounds.centerYPx / ch : prevCenter.y ?? .5
					},
					bottomWidth: keepUnit(currentProps.bottomWidth, bottom * factor),
					topWidth: Math.max(.001, top * factor),
					height: keepUnit(currentProps.height, ch > 0 ? bounds.heightPx / (2 * ch) : halfUvFromProp(currentProps.height, ch, .25)),
					rotation: bounds.rotationDeg
				};
			},
			freeResize: true
		} },
		shapeProps: {
			bottomWidth: {
				default: .35,
				description: "Half-width of the bottom edge",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Bottom Width",
					group: "Shape",
					units: ["%", "px"],
					dimensional: "half-canvas-height"
				}
			},
			topWidth: {
				default: .2,
				description: "Half-width of the top edge",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Top Width",
					group: "Shape"
				}
			},
			height: {
				default: .25,
				description: "Half-height of the trapezoid",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Height",
					group: "Shape",
					units: ["%", "px"],
					dimensional: "half-canvas-height"
				}
			}
		}
	}
});
var Trapezoid_default = componentDefinition;
export { componentDefinition as n, Trapezoid_default as t };
