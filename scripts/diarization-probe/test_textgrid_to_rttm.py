import unittest

from textgrid_to_rttm import convert_textgrid_to_rttm, read_speaker_turns


TEXTGRID = '''File type = "ooTextFile"
Object class = "TextGrid"

xmin = 0
xmax = 3
tiers? <exists>
size = 2
item []:
    item [1]:
        class = "IntervalTier"
        name = "N_SPK_b"
        xmin = 0
        xmax = 3
        intervals: size = 2
        intervals [1]:
            xmin = 0.25
            xmax = 1.5
            text = "第一位说话人"
        intervals [2]:
            xmin = 2
            xmax = 2.25
            text = ""
    item [2]:
        class = "IntervalTier"
        name = "N_SPK_a"
        xmin = 0
        xmax = 3
        intervals: size = 2
        intervals [1]:
            xmin = 1.0
            xmax = 2
            text = "重叠发言"
        intervals [2]:
            xmin = 2
            xmax = 3
            text = "第二位说话人"
'''


class TextGridToRttmTests(unittest.TestCase):
    def test_meeting_id_matches_audio_and_textgrid_names(self):
        from prepare_alimeeting_test import meeting_id

        self.assertEqual(meeting_id("R8002_M8002_MS802.wav"), "8002")
        self.assertEqual(meeting_id("R8002_M8002.TextGrid"), "8002")

    def test_der_accepts_more_hypothesis_speakers_than_reference_speakers(self):
        from run_probe import der

        reference = [(0.0, 1.0, "speaker_1")]
        hypothesis = [(0.0, 1.0, f"pred_{i}") for i in range(9)]
        score = der(reference, hypothesis, 1.0, 0.0, False)
        self.assertEqual(score, 8.0)

    def test_unlabeled_reference_message_matches_parser(self):
        from prepare_alimeeting_test import TEXTGRID_LABEL

        with self.assertRaisesRegex(ValueError, "TextGrid contains no labeled speaker turns"):
            read_speaker_turns('''
item [1]:
    class = "IntervalTier"
    name = "N_SPK1"
    intervals: size = 1
    intervals [1]:
        xmin = 0
        xmax = 1
        text = ""
''')
        self.assertEqual(TEXTGRID_LABEL, "TextGrid contains no labeled speaker turns")

    def test_converts_speech_intervals_and_anonymizes_speakers(self):
        actual = convert_textgrid_to_rttm(TEXTGRID, "sample-01")

        self.assertEqual(
            actual,
            "SPEAKER sample-01 1 0.250000 1.250000 <NA> <NA> speaker_2 <NA> <NA>\n"
            "SPEAKER sample-01 1 1.000000 1.000000 <NA> <NA> speaker_1 <NA> <NA>\n"
            "SPEAKER sample-01 1 2.000000 1.000000 <NA> <NA> speaker_1 <NA> <NA>\n",
        )

    def test_rejects_non_anonymous_sample_id(self):
        with self.assertRaisesRegex(ValueError, "sample ID"):
            convert_textgrid_to_rttm(TEXTGRID, "R8002_M8002")

    def test_rejects_malformed_interval(self):
        malformed = TEXTGRID.replace("xmax = 1.5", "xmax = 0.1", 1)
        with self.assertRaisesRegex(ValueError, "invalid interval"):
            convert_textgrid_to_rttm(malformed, "sample-01")

    def test_rejects_empty_speaker_reference(self):
        empty = TEXTGRID.replace('text = "第一位说话人"', 'text = ""').replace(
            'text = "重叠发言"', 'text = ""'
        ).replace('text = "第二位说话人"', 'text = ""')
        with self.assertRaisesRegex(ValueError, "no labeled speaker turns"):
            convert_textgrid_to_rttm(empty, "sample-01")


if __name__ == "__main__":
    unittest.main()
