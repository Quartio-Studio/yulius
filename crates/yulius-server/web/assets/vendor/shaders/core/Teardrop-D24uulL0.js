import { Ac as defineStd, hs as teardrop } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Teardrop",
	description: "Teardrop — a rounded bulb tapering to a sharp point",
	shape: {
		distance: ({ x, y }, u) => teardrop(x, y, u("radius"), u("height")),
		colorDescription: "Fill color of the teardrop",
		centerDescription: "Center position of the teardrop",
		bounds: { size: {
			width: "radius",
			height: "height"
		} },
		shapeProps: {
			radius: {
				default: .22,
				description: "Radius of the rounded bulb",
				ui: {
					type: "range",
					min: .02,
					max: .6,
					step: .01,
					label: "Width",
					group: "Shape"
				}
			},
			height: {
				default: .4,
				description: "Length from the bulb to the point",
				ui: {
					type: "range",
					min: .1,
					max: .7,
					step: .01,
					label: "Length",
					group: "Shape"
				}
			}
		}
	}
});
var Teardrop_default = componentDefinition;
export { componentDefinition as n, Teardrop_default as t };
