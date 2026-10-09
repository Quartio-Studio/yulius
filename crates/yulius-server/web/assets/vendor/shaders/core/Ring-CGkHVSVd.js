import { Ac as defineStd, ds as ring } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Ring",
	description: "Annular ring (donut) with adjustable radius and band thickness",
	shape: {
		distance: ({ x, y }, u) => ring(x, y, u("radius"), u("thickness")),
		rotatable: false,
		colorDescription: "Fill color of the ring",
		centerDescription: "Center position of the ring",
		bounds: { custom: {
			propBindings: {
				x: {
					prop: "center",
					as: "position-x"
				},
				y: {
					prop: "center",
					as: "position-y"
				}
			},
			computeBounds(props, cw, ch) {
				const radius = props.radius ?? .5;
				const thickness = props.thickness ?? 0;
				const cx = typeof props.center?.x === "number" ? props.center.x : .5;
				const cy = typeof props.center?.y === "number" ? props.center.y : .5;
				const diameter = (radius + thickness) * 2 * ch;
				return {
					centerXPx: cx * cw,
					centerYPx: cy * ch,
					widthPx: diameter,
					heightPx: diameter,
					rotationDeg: 0
				};
			},
			writeBounds(bounds, currentProps, cw, ch) {
				const thickness = currentProps.thickness ?? 0;
				const newOuter = Math.max(0, bounds.widthPx / (2 * ch));
				return {
					center: {
						...currentProps.center ?? {
							x: .5,
							y: .5
						},
						x: bounds.centerXPx / cw,
						y: bounds.centerYPx / ch
					},
					radius: Math.max(0, newOuter - thickness)
				};
			}
		} },
		shapeProps: {
			radius: {
				default: .3,
				description: "Distance from center to the ring's midline in UV space",
				ui: {
					type: "range",
					min: 0,
					max: 1,
					step: .01,
					label: "Radius",
					group: "Shape"
				}
			},
			thickness: {
				default: .07,
				description: "Half-width of the ring band — total ring width is twice this value",
				ui: {
					type: "range",
					min: .005,
					max: .3,
					step: .005,
					label: "Thickness",
					group: "Shape"
				}
			}
		},
		stroke: {
			softnessDescription: "Edge softness for antialiasing (applied to both inner and outer ring edges)",
			strokeThicknessMax: .1,
			strokePositionDescription: "Position of the stroke relative to the ring edge"
		}
	}
});
var Ring_default = componentDefinition;
export { componentDefinition as n, Ring_default as t };
