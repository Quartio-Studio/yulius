import { Ac as defineStd, Ml as p, Pl as edgesPropConfig, rr as wave } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "WaveDistortion",
	role: "warp",
	category: "Distortions",
	description: "Wave-based distortion with multiple waveform types",
	boundingBoxDeclaration: { aspectRatio: null },
	animatedTime: { speed: "speed" },
	props: {
		strength: {
			default: .3,
			description: "Distortion intensity",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		frequency: {
			default: 1,
			description: "Number of bends/waves",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 10,
				step: .1,
				label: "Frequency",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		angle: {
			default: 0,
			description: "Direction of wave distortion in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		waveType: {
			default: "sine",
			description: "Shape of the distortion wave",
			compileTime: true,
			transform: (value) => {
				return {
					"sine": 0,
					"triangle": 1,
					"square": 2,
					"sawtooth": 3,
					"bounce": 4
				}[value] ?? 0;
			},
			ui: {
				type: "select",
				options: [
					{
						label: "Sine",
						value: "sine"
					},
					{
						label: "Triangle",
						value: "triangle"
					},
					{
						label: "Square",
						value: "square"
					},
					{
						label: "Sawtooth",
						value: "sawtooth"
					},
					{
						label: "Bounce",
						value: "bounce"
					}
				],
				label: "Wave Type",
				group: "Effect"
			}
		},
		edges: edgesPropConfig("stretch", "How to handle edges when distortion pushes content out of bounds")
	},
	map: wave({
		strength: p("strength"),
		frequency: p("frequency"),
		angle: p("angle"),
		waveType: p("waveType")
	}),
	missingChildMessage: "You must pass a child component into the WaveDistortion shader."
});
var WaveDistortion_default = componentDefinition;
export { componentDefinition as n, WaveDistortion_default as t };
