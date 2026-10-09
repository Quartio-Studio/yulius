import { Ac as defineStd, dc as mul, ns as arc, ts as APERTURE_TO_HALF_ANGLE } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Arc",
	description: "Pie sector (arc wedge) with adjustable radius and aperture angle",
	shape: {
		distance: ({ x, y }, u) => arc(x, y, u("radius"), mul(u("aperture"), APERTURE_TO_HALF_ANGLE)),
		colorDescription: "Fill color of the arc",
		centerDescription: "Center position of the arc",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .38,
				description: "Radius of the pie sector",
				ui: {
					type: "range",
					min: 0,
					max: 1,
					step: .01,
					label: "Size",
					group: "Shape"
				}
			},
			aperture: {
				default: 270,
				description: "Aperture angle of the wedge in degrees (360 = full circle)",
				ui: {
					type: "range",
					min: 0,
					max: 360,
					step: 1,
					label: "Aperture",
					group: "Shape"
				}
			}
		}
	}
});
var Arc_default = componentDefinition;
export { componentDefinition as n, Arc_default as t };
